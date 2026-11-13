mod fifo;
mod oambug;

use crate::mode::{GbMode, PpuRevision};
use fifo::Pipeline;
pub(crate) use oambug::OamBug;
use std::cmp::Ordering;

/// Which of the two picture engines the PPU runs — a **user-facing mode**,
/// named for what you get rather than for how it is built.
///
/// The two are **not** a quality/appearance choice — palettes, upscaling and
/// every other visual enhancement live in `src/screen/`, above the PPU, and are
/// identical either way. The only axis is *exactness against speed*:
///
/// * **`standard`** ([`RenderMode::Fast`], the default) draws a whole scanline
///   in one go at the mode 3 → 0 transition and gives mode 3 a fixed 172-dot
///   length. A register written part-way along a line is invisible: only its
///   final value is ever read. Every well-behaved game looks identical under
///   it, and it is cheap.
/// * **`identical`** ([`RenderMode::Accurate`]) runs the real pixel pipeline
///   dot by dot — BG fetcher, pixel FIFO, sprite fetches, window restart,
///   `SCX & 7` discard — so a mid-mode-3 write to `BGP`/`SCX`/`LCDC`/`WX` takes
///   effect exactly where hardware puts it, and mode 3 gets its true variable
///   length. This is what the Mealybug Tearoom suite and Mooneye's
///   mid-scanline `acceptance/ppu` tests photograph. It costs about
///   [`RenderMode::IDENTICAL_COST`] times as much CPU.
///
/// The Rust identifiers stay `Fast`/`Accurate` because that is what the two
/// engines *are*; `standard`/`identical` is what a person types and sees.
/// Both spellings parse, so nothing written against the old names breaks.
///
/// Selected per run by `$GB_PPU` or the `ppu` config key; a frontend may also
/// switch it live with [`Gpu::set_render_mode`] (the TUI's `P` key).
#[derive(PartialEq, Eq, Debug, Copy, Clone, Default)]
pub enum RenderMode {
    /// `standard` — whole-scanline renderer, fixed-length mode 3. The default.
    #[default]
    Fast,
    /// `identical` — per-dot pixel FIFO, hardware-exact mid-scanline timing.
    Accurate,
}

/// Environment override for the rendering engine, consulted once per `Gpu`.
pub const PPU_ENV: &str = "GB_PPU";

impl RenderMode {
    /// How much more CPU `identical` costs than `standard`. **Measured, not
    /// chosen:** `testharness/ppuengines.rs` times both engines over the same
    /// 400 frames of dmg-acid2 on every `cargo test --release`, and checks this
    /// constant against what it measured — so it cannot quietly become a lie.
    ///
    /// **It used to be 2.1, and it moved because `standard` got faster, not
    /// because `identical` got slower.** The PSP work rewrote the
    /// whole-scanline renderer to fetch once per tile instead of once per pixel
    /// ([`Gpu::draw_map_run`]) — 3.5x on this box — while `identical`, which
    /// does not use that path at all, did not move: it measures 0.487-0.496
    /// ms/frame before and after, and `standard` 0.234 before against
    /// 0.077-0.080 now.
    ///
    /// **A ratio is two numbers and either can move, so re-run the harness
    /// rather than carrying this one forward.** Five consecutive low-load runs
    /// give 6.20, 6.31, 6.31, 6.24, 6.21; a run taken while this machine was
    /// busy gave 7.75, and one while it was very busy gave 5.56 — which is why
    /// the figure is approximate, is shown with a `~`, and is checked against a
    /// deliberately wide band. See docs/portability.md §3.7.
    ///
    /// Quoted wherever the choice is offered — a player trading speed for
    /// exactness has to be told the price at the moment they trade.
    pub const IDENTICAL_COST: f32 = 6.3;

    /// Every mode, in the order the TUI's `P` key cycles them: the default
    /// first, so a player who taps `P` twice is back where they started.
    pub const ALL: [RenderMode; 2] = [RenderMode::Fast, RenderMode::Accurate];

    /// Parse a user-supplied mode name, case-insensitively.
    ///
    /// `standard` and `identical` are the names; `fast`/`scanline` and
    /// `accurate`/`fifo` are kept as aliases so every existing `config.toml`,
    /// `$GB_PPU` invocation, `Makefile` target and script keeps working.
    pub fn parse(s: &str) -> Option<RenderMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "standard" | "fast" | "scanline" => Some(RenderMode::Fast),
            "identical" | "accurate" | "fifo" => Some(RenderMode::Accurate),
            _ => None,
        }
    }

    /// The mode's name — what a user types, and what the status row shows back.
    pub fn name(self) -> &'static str {
        match self {
            RenderMode::Fast => "standard",
            RenderMode::Accurate => "identical",
        }
    }

    /// The pre-rename spelling, still accepted everywhere. Written into the
    /// baseline headers and the `make` target names so a reader can join the
    /// two vocabularies up.
    pub fn alias(self) -> &'static str {
        match self {
            RenderMode::Fast => "fast",
            RenderMode::Accurate => "accurate",
        }
    }

    /// The status row's value: the mode, and — for the expensive one — its
    /// price, right there beside it.
    pub fn status_label(self) -> String {
        match self {
            RenderMode::Fast => "standard".to_string(),
            RenderMode::Accurate => {
                format!("identical ~{:.1}x", Self::IDENTICAL_COST)
            }
        }
    }

    /// One line saying what this mode gives you and what it costs — the toast
    /// on switching, and the line in the help overlay.
    pub fn describe(self) -> String {
        match self {
            RenderMode::Fast => {
                "standard — whole-scanline picture, full speed (the default)".to_string()
            }
            RenderMode::Accurate => format!(
                "identical — hardware-exact picture, drawn dot by dot; about \
                 {:.1}x the CPU",
                Self::IDENTICAL_COST
            ),
        }
    }

    /// The other mode. `P` in the terminal frontend toggles with this.
    pub fn toggled(self) -> RenderMode {
        match self {
            RenderMode::Fast => RenderMode::Accurate,
            RenderMode::Accurate => RenderMode::Fast,
        }
    }

    /// The names a warning offers back to somebody who mistyped one.
    pub fn choices() -> String {
        "identical, standard (aliases: accurate, fast)".to_string()
    }

    /// The default engine, after `$GB_PPU`. A typo warns on stderr and is
    /// ignored rather than silently switching engine — the same rule
    /// [`crate::mode::Model::resolve`] applies to `$GB_MODEL`.
    pub fn from_env() -> RenderMode {
        match std::env::var(PPU_ENV) {
            Err(_) => RenderMode::default(),
            Ok(v) => match RenderMode::parse(&v) {
                Some(m) => m,
                None => {
                    eprintln!(
                        "{PPU_ENV}={v:?} is not a picture mode ({}) — ignoring",
                        RenderMode::choices()
                    );
                    RenderMode::default()
                }
            },
        }
    }
}

const VRAM_SIZE: usize = 0x4000;
const VOAM_SIZE: usize = 0xA0;
pub const HEIGHT: usize = 144;
pub const WIDTH: usize = 160;

/// Dots of OAM scan (mode 2) at the head of every scanline.
const OAM_DOTS: u32 = 80;
/// Dots of pixel transfer (mode 3). Fixed length here — hardware varies it
/// between 172 and 289 and steals the difference from HBlank.
const DRAW_DOTS: u32 = 172;
/// Dots in one scanline: 80 + 172 + 204 (HBlank).
const LINE_DOTS: u32 = 456;
/// The first VBlank line.
const VBLANK_LINE: u8 = 144;
/// How far ahead of the VBlank interrupt the line-144 OAM strobe starts on
/// **Color hardware**: one M-cycle, i.e. the last four dots of line 143. Zero
/// on a DMG, where the two are simultaneous. See [`Gpu::oam_strobe`].
const VBLANK_STROBE_LEAD: u32 = 4;

/// How far before its own scanline the mode-2 OAM strobe rises, on lines 1-143
/// of the per-dot engine: one M-cycle, so the request appears **part-way
/// through the last M-cycle of the previous line** rather than on a boundary.
/// See [`Gpu::oam_strobe`] for the measurements and the references.
///
/// **This used to be documented as a knob rather than a model, and as the last
/// unresolved conflict between the two PPU suites.** It is neither any more.
/// At `4` five Mooneye `acceptance/ppu/intr_2_*` came out one loop iteration
/// long; at `0` those five passed and eleven Mealybug rows lost pixel-exactness
/// to a uniform four-pixel shift. Both suites are hardware ground truth, and
/// what reconciled them was not this constant: it is that Mooneye's five take
/// the interrupt out of **`HALT`** and Mealybug's ROMs take it out of a **`nop`
/// slide**, and a mid-M-cycle request reaches those two CPUs one M-cycle apart.
/// That is modelled now — [`Gpu::take_stat_irq_mid_mcycle`] and
/// [`crate::cpu::core::Cpu::exec`] — and both suites are satisfied with the
/// lead where it always was.
///
/// Being asserted inside an M-cycle is the *whole* content of this constant on
/// this engine: any value in 1..=4 puts the request in the same M-cycle, which
/// is why SameBoy's "1 T-cycle" and docboy's dot 452/453 are the same model as
/// ours. Only `0` — a boundary-aligned strobe — is a different machine.
const OAM_STROBE_LEAD: u32 = 4;

#[derive(PartialEq, Debug, Copy, Clone)]
enum PrioType {
    Color0,
    PrioFlag,
    Normal,
}

#[derive(Debug)]
pub struct Gpu {
    mode: u8,
    modeclock: u32,
    line: u8,
    lyc: u8,
    lcd_on: bool,
    win_tilemap: u16,
    win_on: bool,
    tilebase: u16,
    bg_tilemap: u16,
    sprite_size: u32,
    sprite_on: bool,
    lcdc0: bool,
    lyc_inte: bool,
    m0_inte: bool,
    m1_inte: bool,
    m2_inte: bool,
    scy: u8,
    scx: u8,
    winy: u8,
    winx: u8,
    wy_trigger: bool,
    wy_pos: i32,
    palbr: u8,
    pal0r: u8,
    pal1r: u8,
    palb: [u8; 4],
    pal0: [u8; 4],
    pal1: [u8; 4],
    pub vram: [u8; VRAM_SIZE],
    pub voam: [u8; VOAM_SIZE],
    cbgpal_inc: bool,
    cbgpal_ind: u8,
    cbgpal: [[[u8; 3]; 4]; 8],
    csprit_inc: bool,
    csprit_ind: u8,
    csprit: [[[u8; 3]; 4]; 8],
    vrambank: usize,
    pub data: Box<[u8; WIDTH * HEIGHT * 4]>,
    bgprio: [PrioType; WIDTH],
    pub updated: bool,
    pub interrupt: u8,
    pub gbmode: GbMode,
    /// Which CGB CPU stepping's PPU this is. Machine configuration, not
    /// cartridge state, so it is deliberately **not** serialised — a state
    /// restores into whatever machine is running. See [`PpuRevision`].
    cgb_ppu_revision: PpuRevision,
    hblanking: bool,
    /// The current level of the **STAT interrupt line** — the OR of every
    /// enabled STAT source. `IF.1` is raised on its 0→1 transition only; see
    /// [`Gpu::refresh_stat_line`].
    stat_line: bool,
    /// Was the most recent `IF.1` request asserted part-way through an
    /// M-cycle? See [`Gpu::take_stat_irq_mid_mcycle`]. Live across an
    /// instruction boundary — a HALTed CPU consumes it on the M-cycle after
    /// the one that raised it — so it is machine state and is serialised.
    stat_irq_mid_mcycle: bool,
    /// The **latched** `LY == LYC` coincidence flag (`STAT` bit 2). It is a
    /// flip-flop clocked by the PPU, not a live comparison: with the LCD off
    /// the comparison clock stops, so the flag keeps whatever it held and a
    /// new `LYC` cannot move it. See [`Gpu::refresh_lyc_match`].
    lyc_match: bool,
    /// Set when `LCDC` bit 7 goes 0→1: the scanline the PPU restarts on has
    /// **no OAM scan**. `STAT` reads mode 0 where every other line reads
    /// mode 2, and the mode-2 STAT source does not strobe. Cleared as soon as
    /// that line reaches mode 3.
    skip_first_oam: bool,
    /// While the CPU is in STOP mode the PPU loses its access to VRAM, so
    /// every scanline it draws is a flat shade instead of the tile map:
    /// `Some(255)` (blank/white) on a DMG, `Some(0)` (black) on a Color.
    /// `None` is both "not stopped" and the Mode-3 exception, where the PPU
    /// keeps the access it already holds and carries on drawing normally.
    /// See [`Gpu::set_stop_fill`].
    stop_fill: Option<u8>,
    /// Which rendering engine is running — see [`RenderMode`]. Read once per
    /// `do_cycle`, never per pixel.
    render_mode: RenderMode,
    /// Whether the whole-scanline renderer generates pixels at all — see
    /// [`Gpu::set_render_pixels`]. `true` in every ordinary build and for every
    /// frontend; `false` only when a caller has *declared* it will not read the
    /// framebuffer.
    ///
    /// **Host declaration, not machine state**, so it is not serialised — the
    /// same call [`Gpu::render_mode`] makes. A save state carries no claim
    /// about whether whoever produced it was looking at the picture.
    render_pixels: bool,
    /// State of the per-dot pixel pipeline. Only [`RenderMode::Accurate`]
    /// touches it; the fast renderer leaves it at its default and pays nothing
    /// for its presence beyond the struct itself.
    pipe: Pipeline,
}

impl Default for Gpu {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpu {
    pub fn new() -> Gpu {
        Gpu {
            mode: 0,
            modeclock: 0,
            line: 0,
            lyc: 0,
            lcd_on: false,
            win_tilemap: 0x9C00,
            win_on: false,
            tilebase: 0x8000,
            bg_tilemap: 0x9C00,
            sprite_size: 8,
            sprite_on: false,
            lcdc0: false,
            lyc_inte: false,
            m2_inte: false,
            m1_inte: false,
            m0_inte: false,
            scy: 0,
            scx: 0,
            winy: 0,
            winx: 0,
            wy_trigger: false,
            wy_pos: -1,
            palbr: 0,
            pal0r: 0,
            pal1r: 1,
            palb: [0; 4],
            pal0: [0; 4],
            pal1: [0; 4],
            vram: [0; VRAM_SIZE],
            voam: [0; VOAM_SIZE],
            data: Box::new([0; HEIGHT * WIDTH * 4]),
            bgprio: [PrioType::Normal; WIDTH],
            updated: false,
            interrupt: 0,
            gbmode: GbMode::Classic,
            cgb_ppu_revision: PpuRevision::from_env(),
            cbgpal_inc: false,
            cbgpal_ind: 0,
            cbgpal: [[[0u8; 3]; 4]; 8],
            csprit_inc: false,
            csprit_ind: 0,
            csprit: [[[0u8; 3]; 4]; 8],
            vrambank: 0,
            hblanking: false,
            stat_line: false,
            stat_irq_mid_mcycle: false,
            lyc_match: false,
            skip_first_oam: false,
            stop_fill: None,
            render_mode: RenderMode::from_env(),
            render_pixels: true,
            pipe: Pipeline::default(),
        }
    }

    /// Which rendering engine this PPU is running — see [`RenderMode`].
    pub fn render_mode(&self) -> RenderMode {
        self.render_mode
    }

    /// Which CGB CPU stepping's PPU this is ([`PpuRevision`]).
    pub fn cgb_ppu_revision(&self) -> PpuRevision {
        self.cgb_ppu_revision
    }

    /// Model a specific CGB CPU stepping's PPU. Ignored on DMG-class hardware,
    /// where both differences the type carries are Color-only.
    pub fn set_cgb_ppu_revision(&mut self, rev: PpuRevision) {
        self.cgb_ppu_revision = rev;
    }

    /// Switch rendering engine. Safe at any point: the two engines share every
    /// register and the framebuffer, and the accurate one rebuilds its
    /// pipeline state at the head of each scanline, so a switch shows up from
    /// the next line onward rather than corrupting the current one.
    pub fn set_render_mode(&mut self, mode: RenderMode) {
        self.render_mode = mode;
    }

    /// Whether pixels are being generated — see [`Gpu::set_render_pixels`].
    pub fn render_pixels(&self) -> bool {
        self.render_pixels
    }

    /// Declare whether the caller will read the framebuffer.
    ///
    /// `false` lets the whole-scanline renderer skip [`Gpu::renderscan`]
    /// entirely: no background fetch, no sprite composite, no 92,160 pixel
    /// stores a frame. On a headless fan-out whose observation comes out of
    /// memory rather than out of the picture, that is pure waste, and it is the
    /// single largest saving available to such a caller.
    ///
    /// **It is opt-in and it stays opt-in.** Pixels are on for every frontend,
    /// every harness and every default build, because the picture is the
    /// product; nothing here makes them *unavailable*, it only lets a caller
    /// that has said it does not want them stop paying for them.
    ///
    /// ## Why this cannot be seen by the cartridge
    ///
    /// The framebuffer is emulator *output*. No Game Boy program can read it
    /// back — [`Gpu::data`] and `bgprio` are reachable from no address — so the
    /// CPU executes exactly the same instructions either way. The one piece of
    /// non-output state rendering advances is the window's internal line
    /// counter (`wy_pos`, incremented by `draw_bg`), and the skip path advances
    /// it identically, so **every** field of the PPU except the picture itself
    /// moves exactly as it would have. `gpu::tests::skipping_pixels_changes_
    /// nothing_the_cartridge_can_see` proves it byte for byte.
    ///
    /// ## The per-dot engine ignores it, deliberately
    ///
    /// Under [`RenderMode::Accurate`] the fetcher *is* the timing: mode 3 lasts
    /// exactly as long as producing the pixels takes, so not producing them
    /// would change what the cartridge observes. That engine therefore renders
    /// whatever this says, and [`Gpu::pixels_effective`] reports it.
    ///
    /// ## Toggling, and what action repetition rests on
    ///
    /// **Turn pixels back on at a frame boundary and the next frame is exact** —
    /// byte-identical to one from a machine that never stopped drawing. That is
    /// the guarantee a policy acting every *k* frames needs: skip the
    /// intermediate frames, ask for the one it will look at.
    /// `gameboy::tests::pixels_turned_back_on_at_a_frame_boundary_give_an_exact_
    /// frame` proves it on a scrolling picture.
    ///
    /// Turning them on *mid*-frame leaves that frame half-drawn — the lines
    /// already past are not revisited — so toggle between `Gameboy::frame`
    /// calls, never inside one.
    pub fn set_render_pixels(&mut self, on: bool) {
        self.render_pixels = on;
    }

    /// Whether pixels are *actually* being produced, which is
    /// [`Gpu::render_pixels`] unless the per-dot engine is running — it always
    /// produces them. See [`Gpu::set_render_pixels`].
    pub fn pixels_effective(&self) -> bool {
        self.render_pixels || self.render_mode == RenderMode::Accurate
    }

    /// The current STAT mode (0 HBlank, 1 VBlank, 2 OAM scan, 3 drawing).
    pub fn mode(&self) -> u8 {
        self.mode
    }

    /// Whether the CPU may reach VRAM (`$8000-$9FFF`) on this dot.
    ///
    /// The PPU holds the VRAM bus for the whole of mode 3, so a CPU read there
    /// returns `$FF` and a write is dropped (Pan Docs, ["VRAM/OAM
    /// accessibility"](https://gbdev.io/pandocs/Accessing_VRAM_and_OAM.html)).
    /// mGBA spells the same rule `if (gb->video.mode != 3)` in `GBLoad8` /
    /// `GBStore8` (`src/gb/memory.c`).
    ///
    /// **Only the [per-dot engine](RenderMode::Accurate) enforces it, and that
    /// is a correctness requirement rather than caution.** Blocking needs two
    /// things at once: a mode-3 length that varies the way hardware's does
    /// (172-289 dots), and a PPU standing on the exact dot of the access. The
    /// batched renderer has neither — its mode 3 is a fixed 172 dots — so
    /// blocking there would silently drop writes a game placed legally in
    /// HBlank, which is strictly worse than not blocking at all. The per-dot
    /// engine has both: its fetcher produces the real length, and
    /// `MemoryManagementUnit::bus_tick` advances it before every access.
    ///
    /// With the LCD off `mode` is 0, so everything is reachable — same as mGBA,
    /// which sets `video->mode = 0` in `GBVideoWriteLCDC` when the enable bit
    /// clears.
    ///
    /// **A read is blocked one dot before a write is.** Mooneye ships the two
    /// measurements as separate ROMs — `lcdon_timing-GS` reads and
    /// `lcdon_write_timing-GS` writes, at the same nineteen offsets — and their
    /// tables disagree at exactly the dots the bus changes hands: at dot 80 of
    /// a normal line a VRAM *read* returns `$FF` while a VRAM *write* still
    /// lands, and the same holds for OAM at dot 0 and dot 80. Only the
    /// acquiring edge is asymmetric; the release at the end of mode 3 is shared.
    /// So the fetcher takes the bus at the top of the dot, but a CPU write
    /// already in flight completes anyway.
    ///
    /// This one takes VRAM at dot 80, one dot before `STAT` reads mode 3. It
    /// does *not* apply on the line the PPU restarts on after `LCDC.7` goes
    /// 0→1 (`skip_first_oam`, where `mode` is 0 rather than 2 and the same test
    /// requires VRAM readable at dot 80): there was no mode 2 to hand over from.
    pub(crate) fn cpu_can_read_vram(&self) -> bool {
        self.cpu_can_write_vram()
            && !(self.render_mode == RenderMode::Accurate
                && self.mode == 2
                && self.modeclock >= OAM_DOTS)
    }

    /// Whether a CPU **write** to VRAM lands on this dot — mode 3 only, which
    /// is the plain Pan Docs rule and the one mGBA spells `if (gb->video.mode
    /// != 3)` in `GBStore8`. See [`Gpu::cpu_can_read_vram`] for the extra dot a
    /// read loses.
    pub(crate) fn cpu_can_write_vram(&self) -> bool {
        self.render_mode != RenderMode::Accurate || self.mode != 3
    }

    /// Whether the CPU may **read** OAM (`$FE00-$FE9F`) on this dot.
    ///
    /// OAM belongs to the PPU for the sprite scan *and* the draw, so modes 2
    /// and 3 both block it; modes 0 and 1 do not (Pan Docs, "VRAM/OAM
    /// accessibility"). mGBA: `if (gb->video.mode < 2)` in `GBLoad8`. See
    /// [`Gpu::cpu_can_read_vram`] for why this is per-dot only, and for the
    /// read-versus-write asymmetry this method's extra clause carries.
    ///
    /// **The scan takes OAM at dot 0, one dot before `STAT` reads mode 2** —
    /// `mode` is moved by the dot loop, which runs after `LY` has advanced, so
    /// the dot that starts a scanline still reads mode 0. Same "ask `LY` and
    /// the dot counter, not `mode`" rule as [`Gpu::oam_strobe`], and
    /// `lcdon_timing-GS` measures it directly: OAM at dot 0 of line 1 must read
    /// `$FF` while `STAT` at that dot reads mode 0.
    ///
    /// The line the PPU restarts on is excluded — it has no OAM scan at all.
    pub(crate) fn cpu_can_read_oam(&self) -> bool {
        if self.render_mode != RenderMode::Accurate {
            return true;
        }
        self.mode < 2
            && !(self.lcd_on
                && self.line < VBLANK_LINE
                && self.modeclock < OAM_DOTS
                && !self.skip_first_oam)
    }

    /// Whether a CPU **write** to OAM lands on this dot.
    ///
    /// The scan and the draw both block it, but each starts a dot later than it
    /// does for a read — see [`Gpu::cpu_can_read_vram`]. The `modeclock` clause
    /// is that dot: with the mode bits already reading 2 but the scan over, a
    /// write still gets through.
    pub(crate) fn cpu_can_write_oam(&self) -> bool {
        if self.render_mode != RenderMode::Accurate {
            return true;
        }
        self.mode < 2 || (self.mode == 2 && self.modeclock >= OAM_DOTS)
    }

    /// Take away (or give back) the PPU's VRAM access for the duration of
    /// STOP mode — see the [`Gpu::stop_fill`] field. `None` restores normal
    /// rendering.
    pub fn set_stop_fill(&mut self, fill: Option<u8>) {
        self.stop_fill = fill;
    }

    pub fn new_cgb() -> Gpu {
        Gpu::new()
    }

    /// Where the CGB boot ROM leaves `BCPS`/`OCPS` after loading the DMG
    /// compatibility palettes: one past the last byte it wrote, auto-increment
    /// still enabled, so they read back `$C8` and `$D0`. Mooneye
    /// `misc/boot_hwio-C` asserts both.
    pub(crate) fn set_boot_palette_index(&mut self) {
        self.cbgpal_ind = 0x08;
        self.cbgpal_inc = true;
        self.csprit_ind = 0x10;
        self.csprit_inc = true;
    }

    /// Serialise the PPU. Everything that drives future rendering is captured,
    /// including the internal `modeclock`/`line` counters and the CGB palette
    /// memory. `bgprio` is *not* saved: it is fully rewritten at the start of
    /// every `renderscan` before being read, so it holds no state across
    /// frames. The `data` framebuffer *is* saved so the visible frame is
    /// correct the instant a state loads.
    ///
    /// [`Gpu::render_mode`] is **not** saved. It is a property of this session's
    /// host — which engine the user asked for — not of the emulated machine, so
    /// loading someone else's state must not silently switch their renderer.
    /// The accurate engine's *pipeline* state is machine state and is saved.
    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.mode);
        w.u32(self.modeclock);
        w.u8(self.line);
        w.u8(self.lyc);
        w.bool(self.lcd_on);
        w.u16(self.win_tilemap);
        w.bool(self.win_on);
        w.u16(self.tilebase);
        w.u16(self.bg_tilemap);
        w.u32(self.sprite_size);
        w.bool(self.sprite_on);
        w.bool(self.lcdc0);
        w.bool(self.lyc_inte);
        w.bool(self.m0_inte);
        w.bool(self.m1_inte);
        w.bool(self.m2_inte);
        w.u8(self.scy);
        w.u8(self.scx);
        w.u8(self.winy);
        w.u8(self.winx);
        w.bool(self.wy_trigger);
        w.i32(self.wy_pos);
        w.u8(self.palbr);
        w.u8(self.pal0r);
        w.u8(self.pal1r);
        w.bytes(&self.palb);
        w.bytes(&self.pal0);
        w.bytes(&self.pal1);
        w.bytes(&self.vram);
        w.bytes(&self.voam);
        w.bool(self.cbgpal_inc);
        w.u8(self.cbgpal_ind);
        Self::save_cpal(w, &self.cbgpal);
        w.bool(self.csprit_inc);
        w.u8(self.csprit_ind);
        Self::save_cpal(w, &self.csprit);
        w.usize(self.vrambank);
        w.bytes(&self.data[..]);
        w.bool(self.updated);
        w.u8(self.interrupt);
        w.u8(self.gbmode.to_u8());
        w.bool(self.hblanking);
        w.bool(self.stat_line);
        w.bool(self.stat_irq_mid_mcycle);
        w.bool(self.lyc_match);
        w.bool(self.skip_first_oam);
        // STOP mode can be live at a frame boundary (the CPU sits there until
        // a button is pressed), so the blanked display has to survive a state.
        w.bool(self.stop_fill.is_some());
        w.u8(self.stop_fill.unwrap_or(0));
        // The pixel pipeline *is* machine state: a state saved part-way through
        // mode 3 has to resume the same scanline, mid-tile-fetch and all.
        // Written unconditionally so a state moves between the two engines —
        // the fast renderer simply never reads it back.
        self.pipe.save_state(w);
    }

    fn save_cpal(w: &mut crate::savestate::Writer, pal: &[[[u8; 3]; 4]; 8]) {
        for p in pal {
            for c in p {
                w.bytes(c);
            }
        }
    }

    fn load_cpal(
        r: &mut crate::savestate::Reader,
        pal: &mut [[[u8; 3]; 4]; 8],
    ) -> Result<(), crate::savestate::StateError> {
        for p in pal.iter_mut() {
            for c in p.iter_mut() {
                r.fill(c)?;
            }
        }
        Ok(())
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.mode = r.u8()?;
        self.modeclock = r.u32()?;
        self.line = r.u8()?;
        self.lyc = r.u8()?;
        self.lcd_on = r.bool()?;
        self.win_tilemap = r.u16()?;
        self.win_on = r.bool()?;
        self.tilebase = r.u16()?;
        self.bg_tilemap = r.u16()?;
        self.sprite_size = r.u32()?;
        self.sprite_on = r.bool()?;
        self.lcdc0 = r.bool()?;
        self.lyc_inte = r.bool()?;
        self.m0_inte = r.bool()?;
        self.m1_inte = r.bool()?;
        self.m2_inte = r.bool()?;
        self.scy = r.u8()?;
        self.scx = r.u8()?;
        self.winy = r.u8()?;
        self.winx = r.u8()?;
        self.wy_trigger = r.bool()?;
        self.wy_pos = r.i32()?;
        self.palbr = r.u8()?;
        self.pal0r = r.u8()?;
        self.pal1r = r.u8()?;
        r.fill(&mut self.palb)?;
        r.fill(&mut self.pal0)?;
        r.fill(&mut self.pal1)?;
        r.fill(&mut self.vram)?;
        r.fill(&mut self.voam)?;
        self.cbgpal_inc = r.bool()?;
        self.cbgpal_ind = r.u8()?;
        Self::load_cpal(r, &mut self.cbgpal)?;
        self.csprit_inc = r.bool()?;
        self.csprit_ind = r.u8()?;
        Self::load_cpal(r, &mut self.csprit)?;
        self.vrambank = r.usize()?;
        r.fill(&mut self.data[..])?;
        self.updated = r.bool()?;
        self.interrupt = r.u8()?;
        self.gbmode = crate::mode::GbMode::from_u8(r.u8()?)?;
        self.hblanking = r.bool()?;
        self.stat_line = r.bool()?;
        self.stat_irq_mid_mcycle = r.bool()?;
        self.lyc_match = r.bool()?;
        self.skip_first_oam = r.bool()?;
        let stopped = r.bool()?;
        let fill = r.u8()?;
        self.stop_fill = if stopped { Some(fill) } else { None };
        self.pipe.load_state(r)?;
        Ok(())
    }

    /// The OAM row the PPU will be scanning `dots` dots from where it stands
    /// now, or `None` if it will not be in mode 2 then.
    ///
    /// The PPU is stepped once per instruction, not once per bus cycle, so a
    /// mid-instruction access has to *predict* forward — `dots` is how far the
    /// bus has already advanced inside the current instruction. Nothing here
    /// mutates the PPU: it is arithmetic on the free-running dot counter.
    fn oam_scan_row_in(&self, dots: i32) -> Option<usize> {
        if !self.lcd_on {
            return None;
        }
        const FRAME_DOTS: i32 = (LINE_DOTS * 154) as i32;
        let now = (self.line as i32) * LINE_DOTS as i32 + self.modeclock as i32;
        let pos = (now + dots).rem_euclid(FRAME_DOTS) as u32;
        let line = pos / LINE_DOTS;
        let dot = pos % LINE_DOTS;
        if line >= VBLANK_LINE as u32 || dot >= OAM_DOTS {
            return None;
        }
        // The line the PPU restarts on after `LCDC` bit 7 goes 0→1 has no OAM
        // scan at all, so nothing is there to corrupt.
        if self.skip_first_oam && line == self.line as u32 {
            return None;
        }
        Some((dot / 4) as usize)
    }

    /// The DMG OAM-corruption bug: the CPU put an OAM address on the bus
    /// `dots` into the current instruction. Corrupts OAM if the PPU is
    /// scanning it right then.
    ///
    /// CGB hardware does not have this defect, even running a monochrome
    /// cartridge, so `ColorAsClassic` is excluded along with `Color`; a Super
    /// Game Boy is DMG silicon and does have it (Pan Docs, ["OAM Corruption
    /// Bug"](https://gbdev.io/pandocs/OAM_Corruption_Bug.html)).
    pub(crate) fn oam_bug(&mut self, kind: OamBug, dots: i32) {
        if !self.gbmode.is_dmg() {
            return;
        }
        if let Some(row) = self.oam_scan_row_in(dots) {
            oambug::corrupt(&mut self.voam, row, kind);
        }
    }

    /// Park the PPU `dot` dots into its 70224-dot frame, as the *boot ROM*
    /// left it.
    ///
    /// The LCD is switched on by the boot ROM, not by the cartridge, so at
    /// `PC=$0100` the PPU is already free-running somewhere in a frame — and
    /// **where** is a property of the boot program, not of the hardware. This
    /// emulator's `LCDC` write in `set_initial` restarts it at line 0, which is
    /// right for the consoles whose `boot_hwio` ROM measures line 0 and wrong
    /// for the ones that measure something else; see
    /// [`crate::mmu::MemoryManagementUnit::set_initial`].
    ///
    /// Deliberately does **not** set `skip_first_oam`: that models the PPU
    /// restarting on an `LCDC` write, and this PPU has been running for frames.
    pub(crate) fn set_boot_position(&mut self, dot: u32) {
        let dot = dot % (LINE_DOTS * 154);
        self.line = (dot / LINE_DOTS) as u8;
        self.modeclock = dot % LINE_DOTS;
        self.mode = if self.line >= VBLANK_LINE {
            1
        } else if self.modeclock <= OAM_DOTS {
            2
        } else if self.modeclock <= OAM_DOTS + DRAW_DOTS {
            3
        } else {
            0
        };
        self.skip_first_oam = false;
        self.lyc_match = self.line == self.lyc;
        self.refresh_stat_line();
    }

    // The PPU mode cycle. See docs/components/ppu.md §"The mode cycle" and Pan Docs:
    // https://gbdev.io/pandocs/Rendering.html  (STAT modes: https://gbdev.io/pandocs/STAT.html)
    pub fn do_cycle(&mut self, ticks: u32) {
        if !self.lcd_on {
            return;
        }
        if self.render_mode == RenderMode::Accurate {
            return self.do_cycle_accurate(ticks);
        }

        let mut ticksleft = ticks;

        while ticksleft > 0 {
            // Stepped in <=80-tick chunks so the 80-dot OAM-scan boundary is never
            // overshot; finer register/LY visibility is not modelled (see ppu.md).
            let curticks = if ticksleft >= OAM_DOTS {
                OAM_DOTS
            } else {
                ticksleft
            };
            self.modeclock += curticks;
            ticksleft -= curticks;

            // A scanline is 456 dots (Pan Docs "Rendering"): 144 visible + 10 VBlank
            // = 154 lines/frame. (The old "114 ticks" was the M-cycle count.)
            if self.modeclock >= LINE_DOTS {
                self.modeclock -= LINE_DOTS;
                self.line = (self.line + 1) % 154;
                // LY moved, so the comparison clock re-latches the coincidence
                // flag and the STAT line may follow it.
                self.refresh_lyc_match();

                // This is a VBlank line
                if self.line >= VBLANK_LINE && self.mode != 1 {
                    self.change_mode(1);
                }
            }

            // Fixed 80 / 172 / 204 dot split for mode 2 (OAM) / 3 (draw) / 0 (HBlank).
            // Hardware mode 3 is variable (172-289) and steals from HBlank; this flat
            // split is a deliberate approximation — see ppu.md "honest approximations".
            if self.line < VBLANK_LINE {
                if self.modeclock <= OAM_DOTS {
                    // The line the PPU restarts on has no OAM scan: `STAT`
                    // keeps reading mode 0 until pixel transfer begins. See
                    // `skip_first_oam`.
                    if self.skip_first_oam {
                        self.check_wy_trigger();
                    } else if self.mode != 2 {
                        self.change_mode(2);
                    }
                } else if self.modeclock <= (OAM_DOTS + DRAW_DOTS) {
                    if self.mode != 3 {
                        self.skip_first_oam = false;
                        self.change_mode(3);
                    }
                } else {
                    // the remaining 204
                    if self.mode != 0 {
                        self.change_mode(0);
                    }
                }
            }
            // Re-sample the line at the end of every chunk. Mode changes and
            // LY increments already refresh it, but the line-144 OAM strobe
            // expires on a plain dot count with no event of its own, and
            // nothing else in VBlank would notice it fall.
            self.refresh_stat_line();
        }
    }

    /// How many **dots** the whole-scanline engine can be advanced in one step
    /// without any decision inside [`Gpu::do_cycle`] coming out differently —
    /// the horizon a halted CPU may be batched over. See
    /// [`MemoryManagementUnit::idle_mcycles`](crate::mmu::MemoryManagementUnit).
    ///
    /// The engine changes state at exactly three dots of a scanline: the first
    /// dot **above** the mode-2 window, the first **above** mode 3, and the
    /// line wrap — plus, on Color hardware, the dot the VBlank OAM strobe leads
    /// by ([`VBLANK_STROBE_LEAD`]). Stopping the batch *on* the change and
    /// letting the ordinary code make it there is what keeps this exact: the
    /// interrupt is folded into `IF` on the same absolute M-cycle it would have
    /// been, so the CPU wakes on the same one.
    ///
    /// Returns 0 when there is no horizon to compute (the per-dot engine, which
    /// has an event on every dot by construction, or the LCD switched off,
    /// where nothing here advances at all).
    pub(crate) fn idle_dots(&self) -> u32 {
        if self.render_mode != RenderMode::Fast || !self.lcd_on {
            return 0;
        }
        let m = self.modeclock;
        let mut next = if m <= OAM_DOTS {
            OAM_DOTS + 1
        } else if m <= OAM_DOTS + DRAW_DOTS {
            OAM_DOTS + DRAW_DOTS + 1
        } else {
            LINE_DOTS
        };
        // The Color console's OAM strobe rises before the line wraps, so that
        // dot is a fourth place the STAT line can move.
        if !self.gbmode.is_dmg() {
            let lead = LINE_DOTS - VBLANK_STROBE_LEAD;
            if m < lead {
                next = next.min(lead);
            }
        }
        next.saturating_sub(m)
    }

    /// Latch the window's frame-long "Y condition" if this scanline's `LY`
    /// matches `WY`. Independent of LCDC bit 5 — see `change_mode`.
    fn check_wy_trigger(&mut self) {
        if !self.wy_trigger && self.line == self.winy {
            self.wy_trigger = true;
            self.wy_pos = -1;
        }
    }

    /// The level of the **STAT interrupt line**: the OR of every enabled STAT
    /// source, as the hardware wires them together.
    ///
    /// From AntonioND's TCAGBD §8.7 "STAT Interrupt" (and Pan Docs
    /// <https://gbdev.io/pandocs/STAT.html>, "STAT interrupt"):
    ///
    /// ```text
    /// line = ((LY = LYC)       AND STAT.ENABLE_LYC)
    ///     OR ((ScreenMode = 0) AND STAT.ENABLE_HBL)
    ///     OR ((ScreenMode = 1) AND STAT.ENABLE_VBL)
    ///     OR (OAM strobe       AND STAT.ENABLE_OAM)
    /// ```
    ///
    /// The last term is [`Gpu::oam_strobe`] rather than "mode = 2" because the
    /// strobe also happens on line 144 — see there.
    ///
    /// **Switching the LCD off does not clear the line** — it stops the PPU,
    /// so the mode-derived terms go away, but the coincidence flip-flop holds
    /// its value and keeps feeding the OR gate. That is what makes
    /// `acceptance/ppu/stat_lyc_onoff` rounds 1 and 2 come out right: with the
    /// LYC source enabled and the flag already set, the program switches the
    /// LCD off, `ei`s, and must see **no** interrupt (the line never fell, so
    /// there is no edge to find) — and then switching the LCD back on with the
    /// flag still set must *also* raise nothing. Forcing the line low while
    /// the LCD is off fails round 2 with a spurious interrupt on re-enable.
    ///
    /// The mode terms *are* gated on `lcd_on`, which is a deliberate choice
    /// rather than a reading of the hardware: `STAT` genuinely reads mode 0
    /// with the LCD off, so an ungated `m0_inte` term would raise a STAT
    /// interrupt every time a game blanks the screen. No test in any suite
    /// covers it, and inventing an interrupt there is the more dangerous of
    /// the two guesses.
    fn stat_signal(&self) -> bool {
        // With no source selected the line is low whatever the PPU is doing,
        // and this is asked once per instruction: on an in-order MIPS the four
        // loads and the OR are cheaper than reaching `lyc_signal`/`oam_strobe`.
        if !(self.lyc_inte || self.m0_inte || self.m1_inte || self.m2_inte) {
            return false;
        }
        (self.lyc_inte && self.lyc_signal())
            || (self.lcd_on
                && ((self.m0_inte && self.hblank_strobe())
                    || (self.m1_inte && self.mode == 1)
                    || (self.m2_inte && self.oam_strobe())))
    }

    /// Is the PPU's **HBlank strobe** — what the mode-0 STAT select bit watches
    /// — currently asserted?
    ///
    /// It is not simply `mode == 0`: **the strobe rises one dot before the
    /// `STAT` mode bits read 0**, on the dot the pixel fetcher pushes its 160th
    /// pixel. Mooneye ships two measurements of the same interval and only this
    /// split satisfies both, which is what makes it a reading rather than a
    /// fudge:
    ///
    /// * `acceptance/ppu/intr_2_0_timing` times the mode-2 **interrupt** to the
    ///   mode-0 **interrupt**, so interrupt-dispatch latency cancels and what is
    ///   left is the distance between the two PPU strobes;
    /// * `acceptance/ppu/intr_2_mode0_timing` times the same mode-2 interrupt to
    ///   the first `STAT` read that *reports* mode 0.
    ///
    /// They disagree by one M-cycle, and the disagreement survives any change to
    /// the mode-2 end (which both share) — so it belongs to the mode-0 end. The
    /// interrupt path sees the fetcher finish; the register's mode field is one
    /// stage behind it. `acceptance/ppu/lcdon_timing-GS` pins the register half
    /// independently: it reads `STAT` at dot 252 of a line and requires mode 3.
    ///
    /// Only the per-dot engine can express this — the batched renderer has no
    /// sub-instruction phase to be one dot ahead of, so for it the strobe is
    /// exactly `mode == 0`.
    fn hblank_strobe(&self) -> bool {
        self.mode == 0
            || (self.render_mode == RenderMode::Accurate
                && self.mode == 3
                && self.pipe.lx as usize >= WIDTH)
    }

    /// Is the PPU's **OAM-scan strobe** — the thing the mode-2 STAT select bit
    /// actually watches — currently asserted?
    ///
    /// It is asserted during mode 2 on every visible line, and *also* on
    /// **line 144 only**, the first VBlank line, where `STAT` already reads
    /// mode 1. Pan Docs <https://gbdev.io/pandocs/STAT.html>: "the STAT OAM
    /// interrupt is also triggered by entering VBlank"; Mooneye's
    /// `acceptance/ppu/vblank_stat_intr-GS` measures that trigger as
    /// *simultaneous* with the VBlank interrupt itself.
    ///
    /// The "line 144 only" half is load-bearing in both directions:
    ///
    /// * without it, a game that enables only the mode-2 source misses one
    ///   interrupt per frame — Mealybug's `m2_win_en_toggle` toggles LCDC.5
    ///   once per fire, so the whole frame's window pattern lands on the wrong
    ///   parity of lines;
    /// * treating it instead as "mode 1 also honours the mode-2 bit" for the
    ///   whole of VBlank *holds the STAT line high* across lines 145-153, so
    ///   the real mode 2 at `LY=0` is blocked and its interrupt arrives a line
    ///   late. `acceptance/ppu/intr_1_2_timing-GS` counts exactly that gap.
    ///
    /// **On lines 1-143 the strobe rises one M-cycle early**
    /// ([`OAM_STROBE_LEAD`]) — and the point of that is not *when* it rises but
    /// *that it rises inside an M-cycle instead of on a boundary*, which is
    /// what makes it the only interrupt source here that reaches a halted CPU
    /// and a running one at different times. See
    /// [`Gpu::take_stat_irq_mid_mcycle`], which is where that difference is
    /// recorded, and [`crate::cpu::core::Cpu::exec`], where it is spent.
    ///
    /// Two references say the same thing from opposite sides, and both put it
    /// inside the M-cycle rather than on its edge:
    ///
    /// * **SameBoy** (`Core/display.c`) sets `mode_for_interrupt = 2` one
    ///   T-cycle before it clears the `STAT` mode bits, commented *"The OAM
    ///   STAT interrupt occurs 1 T-cycle before STAT actually changes, except
    ///   on line 0. PPU glitch?"*;
    /// * **docboy** (`src/docboy/docboy/ppu/ppu.cpp`) calls
    ///   `update_stat_irq_for_oam_mode()` from `hblank_452`, and its
    ///   `update_stat_irq` raises the request one T-cycle later still
    ///   (*"It seems that STAT interrupt is triggered with a delay of 1
    ///   T-Cycle"*) — so dot 453.
    ///
    /// Any of dots 452-455 is the same M-cycle, so all three descriptions are
    /// one model here; `0` would be a different one.
    ///
    /// **The line-0 exclusion is measured, not assumed.** `self.line + 1 <
    /// VBLANK_LINE` gives the lead to lines 1-143 and to no other, so line 0's
    /// strobe stays on the boundary — which is what SameBoy's *"except on line
    /// 0"* says, and what Mealybug's own `m3_bgp_change` source says in its
    /// handler comment (*"line 0 timing is different by 4 cycles, so jump only
    /// when on line 0"*, padding every other line by one taken `jr`).
    ///
    /// The mode-0 end of the line is a separate reading, and the two were once
    /// confused with each other: mode 3 handed over to mode 0 a dot early, and
    /// the mode-0 interrupt and the mode-0 register bits are not the same edge.
    /// See [`Gpu::hblank_strobe`] and the mode-3 exit in `Gpu::accurate_dot`.
    ///
    /// One exclusion remains, and a test insists on it: **line 144 keeps its
    /// strobe at dot 0**, simultaneous with the VBlank interrupt, which is
    /// exactly what `vblank_stat_intr-GS` measures.
    ///
    /// **On Color hardware the strobe starts one M-cycle sooner.** Mooneye
    /// ships the measurement twice, and the two ROMs are the same program with
    /// one `nop` moved: `acceptance/ppu/vblank_stat_intr-GS` (DMG, MGB, SGB)
    /// finds the STAT and VBlank interrupts simultaneous, while
    /// `misc/ppu/vblank_stat_intr-C` (CGB, AGB, AGS) finds STAT **one cycle
    /// before** VBlank — its own header says so. `VBLANK_STROBE_LEAD` is that
    /// cycle; the VBlank flag itself is untouched and still goes up with the
    /// line, so the two consoles differ in exactly the one place hardware does.
    fn oam_strobe(&self) -> bool {
        // The OAM scan owns the first `OAM_DOTS` of every visible line, so ask
        // `LY` and the dot counter rather than `mode`. `mode` is moved by the
        // dot loop, which runs *after* `next_line` has advanced `LY`, so the
        // dot that starts a scanline still reads mode 0 — and since the caller
        // folds `IF` once per M-cycle, "a dot late" is a whole M-cycle late to
        // the CPU. Two things follow, and both are measured:
        //
        // * the strobe stopped and restarted at every line boundary, so lines
        //   1-143 raised **two** STAT interrupts each. It was invisible
        //   because the second edge lands inside the first one's dispatch,
        //   which clears `IF.1` at M4 after it — so the pair collapsed into
        //   one for any program that dispatches immediately;
        // * line 0's interrupt arrived a whole M-cycle after `LY` moved.
        //   Mealybug's `m3_bgp_change` pads its handler by exactly one taken
        //   `jr` on every line *except* line 0, so the residual showed up as
        //   line 0 — and only line 0 — coming out 4 pixels to the right on
        //   every ROM that syncs off the mode-2 source. Measured before: the
        //   handler read `LY` at dot 52 on line 0 against dot 44 on lines 1-2.
        if self.render_mode == RenderMode::Accurate
            && self.line < VBLANK_LINE
            && self.modeclock < OAM_DOTS
            && !self.skip_first_oam
        {
            return true;
        }
        if self.mode == 2
            || (self.line == VBLANK_LINE && self.modeclock < OAM_DOTS)
            || (!self.gbmode.is_dmg()
                && self.line == VBLANK_LINE - 1
                && self.modeclock >= LINE_DOTS - VBLANK_STROBE_LEAD)
        {
            return true;
        }
        // Lines 1-143 only, so this can never overlap the Color console's
        // VBlank lead above (which is line 143's tail). See
        // [`OAM_STROBE_LEAD`] — this is the contested constant.
        OAM_STROBE_LEAD > 0
            && self.render_mode == RenderMode::Accurate
            && self.modeclock >= LINE_DOTS - OAM_STROBE_LEAD
            && self.line + 1 < VBLANK_LINE
            && !self.skip_first_oam
    }

    /// Re-latch the `LY == LYC` coincidence flag and follow the STAT line.
    ///
    /// Only ever called while the PPU is running: with the LCD off the
    /// comparison clock is stopped, so the flag holds its last value and a
    /// write to `LYC` cannot change it (`acceptance/ppu/stat_lyc_onoff`
    /// rounds 1-3 read `STAT` back to prove exactly that).
    fn refresh_lyc_match(&mut self) {
        self.lyc_match = self.line == self.lyc;
        self.refresh_stat_line();
    }

    /// The coincidence comparator's **output**, which is not the same thing as
    /// the latched [`Gpu::lyc_match`]: it is forced low for the first dot of
    /// every scanline, while `LY` is changing.
    ///
    /// `acceptance/ppu/lcdon_timing-GS` reads `STAT` with `LYC = 1` at dot 0 of
    /// line 1 and requires bit 2 **clear**, then again at dot 4 of the same line
    /// and requires it **set** — so the flag cannot simply follow `LY`. It is
    /// only ever a *suppression*: at line 2 dot 0 the same test requires the
    /// flag clear (`LY` is 2, `LYC` still 1), so the comparator goes low with
    /// `LY` and comes back up a dot later, rather than holding the old answer.
    ///
    /// mGBA records the same behaviour from the other side — `_endMode0` in
    /// `src/gb/video.c` updates the LYC bit after the mode bits under the
    /// comment *"LYC stat is delayed 1 T-cycle"*, with a `TODO` for the full
    /// M-cycle in `_endMode1`.
    ///
    /// Per-dot engine only: the batched renderer has no sub-instruction phase
    /// for a one-dot blackout to be visible in.
    fn lyc_signal(&self) -> bool {
        self.lyc_match
            && !(self.render_mode == RenderMode::Accurate
                && self.lcd_on
                && self.modeclock == 0)
    }

    /// Recompute the STAT line and raise `IF.1` on its **0→1 transition only**.
    ///
    /// This is "STAT blocking": there is one interrupt line, not four
    /// independent triggers, so a second source going high while another is
    /// already high raises *nothing*. A game whose handler runs off both the
    /// HBlank and the LYC source sees one interrupt per coincidence, not two —
    /// raster-effect titles split the screen by counting those interrupts, and
    /// an emulator that fires per-source runs their effect twice as fast.
    ///
    /// Every path that can move a term of [`Gpu::stat_signal`] must call this:
    /// a mode change, an `LY` increment, and writes to `STAT`, `LYC` and
    /// `LCDC` (which can switch the whole line off).
    fn refresh_stat_line(&mut self) {
        // Measurement only, compiled out of every shipping build.
        #[cfg(feature = "profile")]
        if crate::profile::stat_skipped() {
            return;
        }
        let now = self.stat_signal();
        if now && !self.stat_line {
            self.interrupt |= 0x02;
            // Record *where inside the CPU's M-cycle* this request appeared.
            // See [`Gpu::stat_irq_mid_mcycle`] — it is the only source in the
            // machine that asserts anywhere but on an M-cycle boundary, and a
            // HALTed CPU is one M-cycle slower to notice it.
            self.stat_irq_mid_mcycle = self.oam_strobe_is_early();
        }
        self.stat_line = now;
    }

    /// Is the mode-2 strobe currently asserted **inside** an M-cycle — the
    /// early window on lines 1-143 that [`OAM_STROBE_LEAD`] describes — rather
    /// than on the boundary every other interrupt source flips on?
    ///
    /// Per-dot engine only; the batched renderer has no sub-instruction phase
    /// for the question to mean anything in.
    fn oam_strobe_is_early(&self) -> bool {
        self.render_mode == RenderMode::Accurate
            && self.m2_inte
            && self.modeclock >= LINE_DOTS - OAM_STROBE_LEAD
            && self.line + 1 < VBLANK_LINE
            && !self.skip_first_oam
    }

    /// Was the most recent `IF.1` request raised **part-way through an
    /// M-cycle** rather than on its boundary?
    ///
    /// Every other interrupt in the machine — VBlank, the mode-0 and mode-1
    /// STAT sources, the timer, the serial port — flips on an M-cycle
    /// boundary, so a CPU sampling the interrupt line once per M-cycle sees it
    /// at the same instant whatever it happens to be doing. The **mode-2 OAM
    /// strobe does not**: it is asserted during the last M-cycle of the
    /// previous scanline (see [`Gpu::oam_strobe`] and [`OAM_STROBE_LEAD`]), and
    /// a CPU sitting in `HALT` is one M-cycle slower to act on it than one
    /// executing instructions. [`crate::cpu::core::Cpu::exec`] is where that is
    /// spent; the whole argument, with both references, is at
    /// [`Gpu::stat_irq_mid_mcycle`]'s consumer.
    pub(crate) fn take_stat_irq_mid_mcycle(&mut self) -> bool {
        std::mem::take(&mut self.stat_irq_mid_mcycle)
    }

    fn change_mode(&mut self, mode: u8) {
        self.mode = mode;

        match self.mode {
            0 => {
                // The whole-scanline renderer draws the line it has just
                // finished. The accurate engine has already painted every
                // pixel of it, one dot at a time, so calling this there would
                // overwrite the line with the very approximation the engine
                // exists to replace — and, worse, silently: `draw_bg`
                // advances the window's line counter as a side effect, so
                // running both moved the window down two rows per scanline.
                if self.render_mode == RenderMode::Fast {
                    self.renderscan();
                }
                self.hblanking = true;
            }
            1 => {
                // Vertical blank
                self.wy_trigger = false;
                self.interrupt |= 0x01;
                self.updated = true;
            }
            2 => {
                // The window's "Y condition" (Pan Docs, "Window mid-frame
                // behavior" → "Window rendering criteria"): *at the beginning
                // of each scanline*, if WY == LY the condition becomes true and
                // stays true for the rest of the frame; it is cleared on
                // VBlank (see mode 1 above).
                //
                // Crucially it does **not** depend on LCDC bit 5. A game may
                // latch the condition on a line where the window is disabled
                // and enable the window later in the same frame — Pan Docs
                // illustrates exactly that with Mealybug's `m2_win_en_toggle`.
                // Testing `win_on` here (as this used to) meant the condition
                // could never latch for such a game and the window never
                // appeared at all. See docs/components/ppu.md.
                //
                // Not modelled: Pan Docs' GBC-only note that *clearing* LCDC.5
                // resets the condition. It does not change `m2_win_en_toggle`
                // (whose CGB references are shade-identical to its DMG one),
                // and modelling it would need the mid-mode-3 PPU this scanline
                // renderer does not have — see docs/mealybug.md.
                self.check_wy_trigger();
            }
            _ => {}
        }
        self.refresh_stat_line();
    }

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0x8000..=0x9FFF => {
                self.vram[(self.vrambank * 0x2000) | (a as usize & 0x1FFF)]
            }
            0xFE00..=0xFE9F => self.voam[a as usize - 0xFE00],
            0xFF40 => {
                (if self.lcd_on { 0x80 } else { 0 })
                    | (if self.win_tilemap == 0x9C00 { 0x40 } else { 0 })
                    | (if self.win_on { 0x20 } else { 0 })
                    | (if self.tilebase == 0x8000 { 0x10 } else { 0 })
                    | (if self.bg_tilemap == 0x9C00 { 0x08 } else { 0 })
                    | (if self.sprite_size == 16 { 0x04 } else { 0 })
                    | (if self.sprite_on { 0x02 } else { 0 })
                    | (if self.lcdc0 { 0x01 } else { 0 })
            }
            0xFF41 => {
                0x80 | (if self.lyc_inte { 0x40 } else { 0 })
                    | (if self.m2_inte { 0x20 } else { 0 })
                    | (if self.m1_inte { 0x10 } else { 0 })
                    | (if self.m0_inte { 0x08 } else { 0 })
                    | (if self.lyc_signal() { 0x04 } else { 0 })
                    | self.mode
            }
            0xFF42 => self.scy,
            0xFF43 => self.scx,
            0xFF44 => self.line,
            0xFF45 => self.lyc,
            0xFF46 => 0, // Write only
            0xFF47 => self.palbr,
            0xFF48 => self.pal0r,
            0xFF49 => self.pal1r,
            0xFF4A => self.winy,
            0xFF4B => self.winx,
            0xFF4C => 0xFF,
            0xFF4E => 0xFF,
            // A DMG has none of these registers at all. A **Color console in
            // compatibility mode** does: `VBK` and the two palette-index
            // registers read back, even though writes to them are ignored (the
            // compatibility palettes are the boot ROM's, not the game's). The
            // palette *data* ports stay closed. All four values are asserted by
            // Mooneye `misc/boot_hwio-C`, which runs a DMG cartridge on a CGB.
            0xFF4F..=0xFF6B if self.gbmode.is_dmg() => 0xFF,
            0xFF50..=0xFF67 => 0xFF,
            0xFF69 | 0xFF6B if self.gbmode != GbMode::Color => 0xFF,
            0xFF4F => self.vrambank as u8 | 0xFE,
            0xFF68 => 0x40 | self.cbgpal_ind | (if self.cbgpal_inc { 0x80 } else { 0 }),
            0xFF69 => {
                let palnum = (self.cbgpal_ind >> 3) as usize;
                let colnum = ((self.cbgpal_ind >> 1) & 0x3) as usize;
                if self.cbgpal_ind & 0x01 == 0x00 {
                    self.cbgpal[palnum][colnum][0]
                        | ((self.cbgpal[palnum][colnum][1] & 0x07) << 5)
                } else {
                    ((self.cbgpal[palnum][colnum][1] & 0x18) >> 3)
                        | (self.cbgpal[palnum][colnum][2] << 2)
                }
            }
            0xFF6A => 0x40 | self.csprit_ind | (if self.csprit_inc { 0x80 } else { 0 }),
            0xFF6B => {
                let palnum = (self.csprit_ind >> 3) as usize;
                let colnum = ((self.csprit_ind >> 1) & 0x3) as usize;
                if self.csprit_ind & 0x01 == 0x00 {
                    self.csprit[palnum][colnum][0]
                        | ((self.csprit[palnum][colnum][1] & 0x07) << 5)
                } else {
                    ((self.csprit[palnum][colnum][1] & 0x18) >> 3)
                        | (self.csprit[palnum][colnum][2] << 2)
                }
            }
            _ => 0xFF,
        }
    }

    /// VRAM bank 0. The address is masked rather than asserted: every caller
    /// derives it from guest-supplied tile data, and a 13-bit wrap is what the
    /// hardware's address lines do anyway.
    fn rbvram0(&self, a: u16) -> u8 {
        debug_assert!((0x8000..0xA000).contains(&a), "rbvram0 outside VRAM");
        self.vram[a as usize & 0x1FFF]
    }
    /// VRAM bank 1 — see [`Gpu::rbvram0`].
    fn rbvram1(&self, a: u16) -> u8 {
        debug_assert!((0x8000..0xA000).contains(&a), "rbvram1 outside VRAM");
        self.vram[0x2000 + (a as usize & 0x1FFF)]
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        match a {
            0x8000..=0x9FFF => {
                self.vram[(self.vrambank * 0x2000) | (a as usize & 0x1FFF)] = v
            }
            0xFE00..=0xFE9F => self.voam[a as usize - 0xFE00] = v,
            0xFF40 => {
                let orig_lcd_on = self.lcd_on;
                self.lcd_on = v & 0x80 == 0x80;
                self.win_tilemap = if v & 0x40 == 0x40 { 0x9C00 } else { 0x9800 };
                self.win_on = v & 0x20 == 0x20;
                self.tilebase = if v & 0x10 == 0x10 { 0x8000 } else { 0x8800 };
                self.bg_tilemap = if v & 0x08 == 0x08 { 0x9C00 } else { 0x9800 };
                self.sprite_size = if v & 0x04 == 0x04 { 16 } else { 8 };
                self.sprite_on = v & 0x02 == 0x02;
                self.lcdc0 = v & 0x01 == 0x01;
                if orig_lcd_on && !self.lcd_on {
                    self.modeclock = 0;
                    self.line = 0;
                    self.mode = 0;
                    self.wy_trigger = false;
                    // `do_cycle` returns early with the LCD off, so it never
                    // gets to clear this — leave it set and a pending HBlank
                    // DMA would move a block on every single `do_cycle`.
                    self.hblanking = false;
                    self.skip_first_oam = false;
                    self.clear_screen();
                }
                if !orig_lcd_on && self.lcd_on {
                    // Enabling the LCD restarts the comparison clock, so the
                    // coincidence flag is re-latched here against the LY the
                    // PPU restarts from — and if that flips it from 0 to 1
                    // with the LYC source enabled, this write is itself the
                    // rising edge (`stat_lyc_onoff` round 4 demands the
                    // interrupt; round 2 demands its *absence* when the flag
                    // was already set).
                    self.lyc_match = self.line == self.lyc;
                    // The PPU restarts *mid-line-0 with no OAM scan*: `STAT`
                    // reads mode 0 and goes straight to mode 3
                    // (`acceptance/ppu/lcdon_timing-GS`: "line 0 starts with
                    // mode 0 and goes straight to mode 3"; `stat_lyc_onoff`
                    // round 1 step 3 reads `STAT` back one instruction after
                    // the enable and demands mode 0).
                    self.mode = 0;
                    self.skip_first_oam = true;
                    self.modeclock = 4;
                }
                self.refresh_stat_line();
            }
            0xFF41 => {
                self.lyc_inte = v & 0x40 == 0x40;
                self.m2_inte = v & 0x20 == 0x20;
                self.m1_inte = v & 0x10 == 0x10;
                self.m0_inte = v & 0x08 == 0x08;
                // Enabling a source whose condition is *already* true raises
                // the line here and now (`acceptance/ppu/stat_lyc_onoff`
                // toggles STAT.6 while LY == LYC and counts the fires);
                // disabling the last high source arms the next rising edge.
                self.refresh_stat_line();
            }
            0xFF42 => self.scy = v,
            0xFF43 => self.scx = v,
            0xFF44 => {} // Read-only
            0xFF45 => {
                self.lyc = v;
                // With the LCD off the comparison clock is stopped: the flag
                // keeps its last value and this write is invisible until the
                // PPU is switched back on (`stat_lyc_onoff` rounds 1-3).
                if self.lcd_on {
                    self.refresh_lyc_match();
                }
            }
            // $FF46 is OAM DMA and never reaches here — the MMU claims it.
            0xFF46 => {}
            0xFF47 => {
                self.palbr = v;
                self.update_pal();
            }
            0xFF48 => {
                self.pal0r = v;
                self.update_pal();
            }
            0xFF49 => {
                self.pal1r = v;
                self.update_pal();
            }
            0xFF4A => self.winy = v,
            0xFF4B => self.winx = v,
            0xFF4C => {}
            0xFF4E => {}
            0xFF4F..=0xFF6B if self.gbmode != GbMode::Color => {}
            0xFF4F => self.vrambank = (v & 0x01) as usize,
            0xFF68 => {
                self.cbgpal_ind = v & 0x3F;
                self.cbgpal_inc = v & 0x80 == 0x80;
            }
            0xFF69 => {
                let palnum = (self.cbgpal_ind >> 3) as usize;
                let colnum = ((self.cbgpal_ind >> 1) & 0x03) as usize;
                if self.cbgpal_ind & 0x01 == 0x00 {
                    self.cbgpal[palnum][colnum][0] = v & 0x1F;
                    self.cbgpal[palnum][colnum][1] =
                        (self.cbgpal[palnum][colnum][1] & 0x18) | (v >> 5);
                } else {
                    self.cbgpal[palnum][colnum][1] =
                        (self.cbgpal[palnum][colnum][1] & 0x07) | ((v & 0x3) << 3);
                    self.cbgpal[palnum][colnum][2] = (v >> 2) & 0x1F;
                }
                if self.cbgpal_inc {
                    self.cbgpal_ind = (self.cbgpal_ind + 1) & 0x3F;
                };
            }
            0xFF6A => {
                self.csprit_ind = v & 0x3F;
                self.csprit_inc = v & 0x80 == 0x80;
            }
            0xFF6B => {
                let palnum = (self.csprit_ind >> 3) as usize;
                let colnum = ((self.csprit_ind >> 1) & 0x03) as usize;
                if self.csprit_ind & 0x01 == 0x00 {
                    self.csprit[palnum][colnum][0] = v & 0x1F;
                    self.csprit[palnum][colnum][1] =
                        (self.csprit[palnum][colnum][1] & 0x18) | (v >> 5);
                } else {
                    self.csprit[palnum][colnum][1] =
                        (self.csprit[palnum][colnum][1] & 0x07) | ((v & 0x3) << 3);
                    self.csprit[palnum][colnum][2] = (v >> 2) & 0x1F;
                }
                if self.csprit_inc {
                    self.csprit_ind = (self.csprit_ind + 1) & 0x3F;
                };
            }
            // Anything else in this range is unmapped: hardware drops the
            // write. Aborting instead turned a stray guest DMA into process
            // death and a lost battery save (see `src/mbc/battery.rs`).
            _ => {}
        }
    }

    fn clear_screen(&mut self) {
        for v in self.data.iter_mut() {
            *v = 255;
        }
        self.updated = true;
    }

    fn update_pal(&mut self) {
        for i in 0..4 {
            self.palb[i] = Gpu::get_monochrome_pal_val(self.palbr, i);
            self.pal0[i] = Gpu::get_monochrome_pal_val(self.pal0r, i);
            self.pal1[i] = Gpu::get_monochrome_pal_val(self.pal1r, i);
        }
    }

    fn get_monochrome_pal_val(value: u8, index: usize) -> u8 {
        match (value >> (2 * index)) & 0x03 {
            0 => 255,
            1 => 192,
            2 => 96,
            _ => 0,
        }
    }

    fn renderscan(&mut self) {
        // Measurement only, compiled out of every shipping build. Guest-
        // invisible: the framebuffer is emulator output and no cartridge can
        // read it back. See `crate::profile`.
        #[cfg(feature = "profile")]
        let measuring = crate::profile::pixels_skipped();
        #[cfg(not(feature = "profile"))]
        let measuring = false;

        // STOP mode: the PPU has been cut off from VRAM, so it draws a flat
        // shade rather than the tile map — see `stop_fill`. It advances no
        // window line, so the skip below sits *after* it and the two paths
        // agree on that too.
        if let Some(shade) = self.stop_fill {
            if !(measuring || !self.render_pixels) {
                for x in 0..WIDTH {
                    self.setcolor(x, shade);
                    self.bgprio[x] = PrioType::Normal;
                }
            }
            return;
        }
        // The caller has declared it will not read the picture — see
        // [`Gpu::set_render_pixels`] — or the measurement switch has, which is
        // the same claim made by `crate::profile` and compiled out of every
        // shipping build. Everything below this line writes only `data` and
        // `bgprio`, both of which are emulator output, so the one thing that
        // must still happen is the window's line counter.
        if measuring || !self.render_pixels {
            self.advance_window_line();
            return;
        }
        // The white pre-clear exists for the pixels [`Gpu::draw_bg`] declines to
        // draw, and there are none unless the background is switched off: on a
        // Color console, or on a DMG with `LCDC` bit 0 set, `draw_bg` writes
        // every one of the 160 pixels and every `bgprio` entry, so clearing
        // first is 640 stores a scanline — 92,160 a frame — thrown away. Same
        // picture either way; the condition is `draw_bg`'s own `drawbg`.
        if !(self.gbmode == GbMode::Color || self.lcdc0) {
            for x in 0..WIDTH {
                self.setcolor(x, 255);
                self.bgprio[x] = PrioType::Normal;
            }
        }
        self.draw_bg();
        self.draw_sprites();
    }

    /// Byte offset of the current scanline in [`Gpu::data`].
    ///
    /// The `min` is not defensive padding, it is what lets the compiler drop
    /// the bounds check on every pixel store: `line` is a `u8`, so without it
    /// the index range the optimiser must consider runs past the end of a
    /// 160x144 framebuffer and it has to test each write. Rendering only ever
    /// happens on a visible line, so the clamp can never fire.
    #[inline(always)]
    fn row_base(&self) -> usize {
        (self.line as usize).min(HEIGHT - 1) * WIDTH * 4
    }

    fn setcolor(&mut self, x: usize, color: u8) {
        let base = self.row_base() + x * 4;
        self.data[base] = color;
        self.data[base + 1] = color;
        self.data[base + 2] = color;
    }

    fn setrgb(&mut self, x: usize, r: u8, g: u8, b: u8) {
        // Gameboy Color RGB correction
        // Taken from the Gambatte emulator
        // assume r, g and b are between 0 and 1F
        let baseidx = self.row_base() + x * 4;

        let r = r as u32;
        let g = g as u32;
        let b = b as u32;

        self.data[baseidx] = ((r * 13 + g * 2 + b) >> 1) as u8;
        self.data[baseidx + 1] = ((g * 3 + b) << 1) as u8;
        self.data[baseidx + 2] = ((r * 3 + g * 2 + b * 11) >> 1) as u8;
    }

    /// Draw one horizontal run of background or window pixels from one tilemap.
    ///
    /// Split out of [`Gpu::draw_bg`] because a scanline is at most **two** such
    /// runs — background up to the window's left edge, window from there on —
    /// and which one a pixel belongs to is therefore decided once a line rather
    /// than once a pixel. Everything is passed in by disjoint borrow so the
    /// framebuffer row can be a slice: one bounds check for the line instead of
    /// three per pixel.
    ///
    /// `coord` is the map-space X of `x_start` and advances with `x`; the tile
    /// changes every eight of them, which is the whole point — the tilemap
    /// byte, the Color attribute byte and both tile-data bytes are read once
    /// per tile instead of once per pixel.
    // Two arrays with different strides are indexed by the same `x` (one entry
    // of `bgprio`, four bytes of `row`), so an iterator rewrite would need a
    // zip of chunks and read worse, not better.
    #[allow(clippy::too_many_arguments, clippy::needless_range_loop)]
    fn draw_map_run(
        row: &mut [u8],
        bgprio: &mut [PrioType; WIDTH],
        vram: &[u8],
        cbgpal: &[[[u8; 3]; 4]; 8],
        palb: [u8; 4],
        color: bool,
        tilebase: u16,
        tilemapbase: u16,
        tiley: u16,
        pixely: u16,
        tilex_mask: u16,
        x_start: usize,
        x_end: usize,
        coord_at_start: u32,
    ) {
        let mut b1 = 0u8;
        let mut b2 = 0u8;
        let mut palnr = 0usize;
        let mut xflip = false;
        let mut prio = false;

        for x in x_start..x_end {
            let coord = coord_at_start.wrapping_add((x - x_start) as u32);
            let pixelx = (coord & 0x07) as u8;

            if pixelx == 0 || x == x_start {
                let tilex = ((coord >> 3) as u16) & tilex_mask;
                let mapaddr = (tilemapbase + tiley * 32 + tilex) as usize & 0x1FFF;
                let tilenr = vram[mapaddr];

                let yflip;
                let vram1;
                if color {
                    let flags = vram[0x2000 + mapaddr] as usize;
                    palnr = flags & 0x07;
                    vram1 = flags & (1 << 3) != 0;
                    xflip = flags & (1 << 5) != 0;
                    yflip = flags & (1 << 6) != 0;
                    prio = flags & (1 << 7) != 0;
                } else {
                    palnr = 0;
                    vram1 = false;
                    xflip = false;
                    yflip = false;
                    prio = false;
                }

                let tileaddress = tilebase
                    + (if tilebase == 0x8000 {
                        tilenr as u16
                    } else {
                        (tilenr as i8 as i16 + 128) as u16
                    }) * 16;

                let a0 = match yflip {
                    false => tileaddress + (pixely * 2),
                    true => tileaddress + (14 - (pixely * 2)),
                } as usize
                    & 0x1FFF;
                let bank = if vram1 { 0x2000 } else { 0 };
                b1 = vram[bank + a0];
                b2 = vram[bank + ((a0 + 1) & 0x1FFF)];
            }

            let xbit = match xflip {
                true => pixelx,
                false => 7 - pixelx,
            } as u32;
            let colnr = if b1 & (1 << xbit) != 0 { 1 } else { 0 }
                | if b2 & (1 << xbit) != 0 { 2 } else { 0 };

            bgprio[x] = if colnr == 0 {
                PrioType::Color0
            } else if prio {
                PrioType::PrioFlag
            } else {
                PrioType::Normal
            };

            let o = x * 4;
            if color {
                // The Gambatte correction of `Gpu::setrgb`, inlined so the row
                // slice can be reused; the arithmetic is that function's.
                let r = cbgpal[palnr][colnr][0] as u32;
                let g = cbgpal[palnr][colnr][1] as u32;
                let b = cbgpal[palnr][colnr][2] as u32;
                row[o] = ((r * 13 + g * 2 + b) >> 1) as u8;
                row[o + 1] = ((g * 3 + b) << 1) as u8;
                row[o + 2] = ((r * 3 + g * 2 + b * 11) >> 1) as u8;
            } else {
                let c = palb[colnr];
                row[o] = c;
                row[o + 1] = c;
                row[o + 2] = c;
            }
        }
    }

    /// Draw this scanline's background and window.
    ///
    /// **A scanline is two runs, and the fetch is per tile.** Which of the
    /// background or the window a pixel comes from is decided by one comparison
    /// against the window's left edge, once a line, rather than by two tests a
    /// pixel; and eight neighbouring pixels share a tile, so four VRAM reads
    /// serve them all instead of thirty-two. Same picture, and it was measured:
    /// pixel rendering on the PSP fell from 8.29 ms a frame to 5.30 with the
    /// per-tile fetch alone (`docs/portability.md` §3.7).
    /// The window's internal line counter, advanced exactly as [`Gpu::draw_bg`]
    /// advances it.
    ///
    /// This is the *only* thing rendering a scanline changes that is not the
    /// picture, which is why it is a function: the skip path in
    /// [`Gpu::renderscan`] calls it and nothing else, so a run with pixels off
    /// leaves every PPU field byte-identical to a run with them on. The two
    /// call sites must not drift — see
    /// `tests::skipping_pixels_changes_nothing_the_cartridge_can_see`.
    #[inline]
    fn advance_window_line(&mut self) -> i32 {
        if self.win_on && self.wy_trigger && self.winx <= 166 {
            self.wy_pos += 1;
            self.wy_pos
        } else {
            -1
        }
    }

    fn draw_bg(&mut self) {
        let drawbg = self.gbmode == GbMode::Color || self.lcdc0;

        let winy = self.advance_window_line();

        if winy < 0 && !drawbg {
            return;
        }

        let wintiley = (winy as u16 >> 3) & 31;

        let bgy = self.scy.wrapping_add(self.line);
        let bgtiley = (bgy as u16 >> 3) & 31;

        // The window owns every pixel from its left edge on; the background
        // owns the ones before it. `winx0` is that edge in screen coordinates,
        // and the whole line is the background when the window is not active.
        let winx0 = self.winx as i32 - 7;
        let wstart = if winy >= 0 {
            winx0.clamp(0, WIDTH as i32) as usize
        } else {
            WIDTH
        };

        let color = self.gbmode == GbMode::Color;
        let win_tilemap = self.win_tilemap;
        let bg_tilemap = self.bg_tilemap;
        let tilebase = self.tilebase;
        let scx = self.scx as u32;
        let palb = self.palb;
        let base = self.row_base();

        // Disjoint field borrows: the row slice is `&mut self.data`, the tile
        // data `&self.vram`, the priority map `&mut self.bgprio`.
        let vram = &self.vram;
        let cbgpal = &self.cbgpal;
        let bgprio = &mut self.bgprio;
        let row = &mut self.data[base..base + WIDTH * 4];

        if drawbg && wstart > 0 {
            Self::draw_map_run(
                row,
                bgprio,
                vram,
                cbgpal,
                palb,
                color,
                tilebase,
                bg_tilemap,
                bgtiley,
                bgy as u16 & 0x07,
                31,
                0,
                wstart,
                scx,
            );
        }
        if wstart < WIDTH {
            Self::draw_map_run(
                row,
                bgprio,
                vram,
                cbgpal,
                palb,
                color,
                tilebase,
                win_tilemap,
                wintiley,
                winy as u16 & 0x07,
                0xFFFF,
                wstart,
                WIDTH,
                (wstart as i32 - winx0) as u32,
            );
        }
    }

    fn draw_sprites(&mut self) {
        if !self.sprite_on {
            return;
        }

        let line = self.line as i32;
        let sprite_size = self.sprite_size as i32;

        // OAM straight from the array, not through `Gpu::rb`: the scan reads two
        // bytes of each of the forty objects on every visible line, which is
        // 11,520 address-dispatch calls a frame for four bytes that are always
        // in `voam`. `Gpu::rb` returns exactly `voam[a - 0xFE00]` for this
        // range, so the picture cannot differ.
        let mut sprites_to_draw = [(0, 0, 0); 10];
        let mut sidx = 0;
        for index in 0..40u8 {
            let o = index as usize * 4;
            let spritey = self.voam[o] as i32 - 16;
            if line < spritey || line >= spritey + sprite_size {
                continue;
            }
            let spritex = self.voam[o + 1] as i32 - 8;
            sprites_to_draw[sidx] = (spritex, spritey, index);
            sidx += 1;
            if sidx >= 10 {
                break;
            }
        }
        if self.gbmode == GbMode::Color {
            sprites_to_draw[..sidx].sort_unstable_by(cgb_sprite_order);
        } else {
            sprites_to_draw[..sidx].sort_unstable_by(dmg_sprite_order);
        }

        for &(spritex, spritey, i) in &sprites_to_draw[..sidx] {
            if spritex < -7 || spritex >= (WIDTH as i32) {
                continue;
            }

            let tilenum = (self.voam[i as usize * 4 + 2]
                & (if self.sprite_size == 16 { 0xFE } else { 0xFF }))
                as u16;
            let flags = self.voam[i as usize * 4 + 3] as usize;
            let usepal1: bool = flags & (1 << 4) != 0;
            let xflip: bool = flags & (1 << 5) != 0;
            let yflip: bool = flags & (1 << 6) != 0;
            let belowbg: bool = flags & (1 << 7) != 0;
            let c_palnr = flags & 0x07;
            let c_vram1: bool = flags & (1 << 3) != 0;

            let tiley: u16 = if yflip {
                (sprite_size - 1 - (line - spritey)) as u16
            } else {
                (line - spritey) as u16
            };

            let tileaddress = 0x8000u16 + tilenum * 16 + tiley * 2;
            let (b1, b2) = if c_vram1 && self.gbmode == GbMode::Color {
                (self.rbvram1(tileaddress), self.rbvram1(tileaddress + 1))
            } else {
                (self.rbvram0(tileaddress), self.rbvram0(tileaddress + 1))
            };

            'xloop: for x in 0..8 {
                if spritex + x < 0 || spritex + x >= (WIDTH as i32) {
                    continue;
                }

                let xbit = 1 << (if xflip { x } else { 7 - x } as u32);
                let colnr = (if b1 & xbit != 0 { 1 } else { 0 })
                    | (if b2 & xbit != 0 { 2 } else { 0 });
                if colnr == 0 {
                    continue;
                }

                if self.gbmode == GbMode::Color {
                    if self.lcdc0
                        && (self.bgprio[(spritex + x) as usize] == PrioType::PrioFlag
                            || (belowbg
                                && self.bgprio[(spritex + x) as usize]
                                    != PrioType::Color0))
                    {
                        continue 'xloop;
                    }
                    let r = self.csprit[c_palnr][colnr][0];
                    let g = self.csprit[c_palnr][colnr][1];
                    let b = self.csprit[c_palnr][colnr][2];
                    self.setrgb((spritex + x) as usize, r, g, b);
                } else {
                    if belowbg && self.bgprio[(spritex + x) as usize] != PrioType::Color0
                    {
                        continue 'xloop;
                    }
                    let color = if usepal1 {
                        self.pal1[colnr]
                    } else {
                        self.pal0[colnr]
                    };
                    self.setcolor((spritex + x) as usize, color);
                }
            }
        }
    }

    /// True on the *entry* into HBlank — one HDMA block per scanline.
    ///
    /// This is a **latch**, cleared by [`Gpu::clear_hblank_latch`] once the MMU
    /// has had its look, rather than at the top of every [`Gpu::do_cycle`] as
    /// it used to be. The accurate engine is stepped several times per CPU
    /// instruction (once per bus access), so a self-clearing flag could be
    /// raised and cleared again *between* two of the MMU's HDMA checks and the
    /// block would silently never move. Same observable behaviour for the fast
    /// renderer, which is stepped exactly once per instruction.
    pub fn may_hdma(&self) -> bool {
        self.hblanking
    }

    /// Consume the HBlank-entry latch — see [`Gpu::may_hdma`]. Called by the
    /// MMU immediately after it has decided whether to move an HDMA block.
    pub fn clear_hblank_latch(&mut self) {
        self.hblanking = false;
    }

    /// Whether the PPU is in mode 0 as the CPU can see it. With the LCD off
    /// STAT's mode bits read 0, and an HBlank DMA armed then moves a block
    /// straight away (SameSuite `dma/hdma_lcd_off`).
    pub fn in_hblank(&self) -> bool {
        !self.lcd_on || self.mode == 0
    }
}

// Functions to determine the order of sprites. Input is a tuple x-coord, OAM position
// These function ensures that sprites with a higher priority are 'larger'
fn dmg_sprite_order(a: &(i32, i32, u8), b: &(i32, i32, u8)) -> Ordering {
    // DMG order: prioritize on x-coord, and then by OAM position.
    if a.0 != b.0 {
        return b.0.cmp(&a.0);
    }
    b.2.cmp(&a.2)
}

fn cgb_sprite_order(a: &(i32, i32, u8), b: &(i32, i32, u8)) -> Ordering {
    // CGB order: only prioritize based on OAM position.
    b.2.cmp(&a.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOTS_PER_LINE: u32 = 456;

    /// The mode name round-trips through the spelling a user types, and an
    /// unknown one is rejected rather than quietly meaning "standard". A silent
    /// fallback here would publish conformance scores from the wrong renderer.
    #[test]
    fn a_render_mode_is_named_by_the_word_the_user_types() {
        for (text, mode) in [
            ("standard", RenderMode::Fast),
            ("IDENTICAL", RenderMode::Accurate),
            (" identical ", RenderMode::Accurate),
        ] {
            assert_eq!(RenderMode::parse(text), Some(mode), "{text:?}");
        }
        assert_eq!(RenderMode::parse("sideways"), None);
        assert_eq!(RenderMode::parse(""), None);
        assert_eq!(RenderMode::Fast.name(), "standard");
        assert_eq!(RenderMode::Accurate.name(), "identical");
        for mode in RenderMode::ALL {
            assert_eq!(
                RenderMode::parse(mode.name()),
                Some(mode),
                "the name shown in the status row must be a name we accept back"
            );
        }
    }

    /// **The pre-rename spellings must keep working.** `config.toml` files,
    /// shell aliases, the `Makefile`'s `GB_PPU=accurate` targets and every
    /// script anyone has written say `fast`/`accurate`; renaming the modes must
    /// cost none of them.
    #[test]
    fn the_old_engine_names_are_still_accepted() {
        for (text, mode) in [
            ("fast", RenderMode::Fast),
            ("scanline", RenderMode::Fast),
            ("ACCURATE", RenderMode::Accurate),
            ("fifo", RenderMode::Accurate),
            (" accurate ", RenderMode::Accurate),
        ] {
            assert_eq!(RenderMode::parse(text), Some(mode), "{text:?}");
        }
        for mode in RenderMode::ALL {
            assert_eq!(RenderMode::parse(mode.alias()), Some(mode));
        }
        // A warning must offer back every spelling it accepts, so somebody who
        // typed one of the old ones is not told it does not exist.
        let choices = RenderMode::choices();
        for word in ["identical", "standard", "accurate", "fast"] {
            assert!(choices.contains(word), "{choices:?} omits {word:?}");
        }
    }

    /// `P` toggles, and two taps are a no-op — an option a player can get lost
    /// inside is one they will not touch twice.
    #[test]
    fn toggling_the_picture_mode_twice_returns_to_where_it_started() {
        for mode in RenderMode::ALL {
            assert_ne!(mode.toggled(), mode);
            assert_eq!(mode.toggled().toggled(), mode);
        }
        assert_eq!(RenderMode::ALL[0], RenderMode::default());
    }

    /// **The cost is stated where the choice is made.** A player trading speed
    /// for exactness has to see the price, and only the expensive mode carries
    /// one — labelling the default "1.0x" would imply it is a compromise.
    #[test]
    fn the_expensive_mode_says_what_it_costs() {
        let acc = RenderMode::Accurate;
        // Read the figure off the constant rather than repeating it, so a
        // re-measurement moves one number and this stays true.
        let cost = format!("{:.1}x", RenderMode::IDENTICAL_COST);
        assert!(acc.status_label().contains(&cost), "{}", acc.status_label());
        assert!(acc.describe().contains(&cost), "{}", acc.describe());
        assert_eq!(RenderMode::Fast.status_label(), "standard");
        assert!(!RenderMode::Fast.describe().contains('x'));
        for mode in RenderMode::ALL {
            assert!(mode.status_label().starts_with(mode.name()));
            assert!(mode.describe().starts_with(mode.name()));
        }
    }

    /// A PPU with the LCD on, sitting at the start of line 0.
    fn lcd_on(stat: u8, wy: u8) -> Gpu {
        let mut gpu = Gpu::new();
        gpu.wb(0xFF41, stat);
        gpu.wb(0xFF4A, wy);
        gpu.wb(0xFF40, 0x80); // LCD on; enters mode 2 on line 0
        gpu
    }

    fn advance_lines(gpu: &mut Gpu, n: u32) {
        for _ in 0..n {
            gpu.do_cycle(DOTS_PER_LINE);
        }
    }

    /// Step `dots` one M-cycle at a time and count how many *separate* times
    /// the PPU raised `IF.1`, clearing the flag between steps exactly as a CPU
    /// servicing the interrupt would. Counting raises rather than reading the
    /// flag once is the whole point: a source-per-trigger PPU produces several
    /// where hardware produces one.
    fn count_stat_raises(gpu: &mut Gpu, dots: u32) -> u32 {
        let mut n = 0;
        for _ in 0..dots / 4 {
            gpu.interrupt = 0;
            gpu.do_cycle(4);
            if gpu.interrupt & 0x02 != 0 {
                n += 1;
            }
        }
        n
    }

    /// The window's frame-long "Y condition" is latched by WY == LY *at the
    /// start of a scanline* and does not consult LCDC bit 5 (Pan Docs, "Window
    /// rendering criteria"). Requiring the window to be enabled at latch time
    /// meant a game that turned the window on later in the frame never saw it
    /// at all — the failure Mealybug's `m2_win_en_toggle` renders as a blank
    /// screen.
    #[test]
    fn the_wy_condition_latches_while_the_window_is_disabled() {
        let mut gpu = lcd_on(0x00, 5);
        assert!(
            !gpu.win_on,
            "the window must be off for this to prove anything"
        );

        advance_lines(&mut gpu, 4);
        assert_eq!(gpu.line, 4);
        assert!(!gpu.wy_trigger, "WY(5) != LY(4), nothing to latch yet");

        advance_lines(&mut gpu, 1);
        assert_eq!(gpu.line, 5);
        assert!(
            gpu.wy_trigger,
            "WY == LY must latch the Y condition even with LCDC bit 5 clear"
        );

        // ...and it survives to the end of the frame, then clears on VBlank.
        advance_lines(&mut gpu, 100);
        assert!(
            gpu.wy_trigger,
            "the Y condition is latched for the whole frame"
        );
        advance_lines(&mut gpu, 39);
        assert!(gpu.line >= 144, "should be in VBlank by now");
        assert!(!gpu.wy_trigger, "VBlank clears the Y condition");
    }

    /// Entering VBlank strobes the OAM source too, so a program that selects
    /// only the mode-2 bit still gets a STAT interrupt at `LY=144`. Mooneye's
    /// `acceptance/ppu/vblank_stat_intr-GS` measures it as *simultaneous* with
    /// the VBlank interrupt itself.
    #[test]
    fn entering_vblank_raises_stat_for_the_mode_2_source_too() {
        for (stat, want_stat_at_144) in [(0x20u8, true), (0x00u8, false)] {
            let mut gpu = lcd_on(stat, 0xFF);
            advance_lines(&mut gpu, 143);
            assert_eq!(gpu.line, 143);

            gpu.interrupt = 0;
            advance_lines(&mut gpu, 1);
            assert_eq!(gpu.line, 144, "should have just entered VBlank");
            assert_eq!(
                gpu.interrupt & 0x01,
                0x01,
                "VBlank interrupt is unconditional"
            );
            assert_eq!(
                gpu.interrupt & 0x02 != 0,
                want_stat_at_144,
                "STAT=${stat:02X}: mode-2 select must raise STAT on entering mode 1"
            );
        }
    }

    /// **STAT blocking.** There is one interrupt line, not four triggers: a
    /// second source going high while another is already high raises nothing.
    /// On a line where `LY == LYC` with both the LYC and the HBlank source
    /// selected, hardware fires **once** — Mooneye's
    /// `acceptance/ppu/stat_irq_blocking` builds a whole frame out of exactly
    /// that arrangement and fails on any extra interrupt. Raising per source
    /// (as this used to) gives two, which runs a raster split twice as fast as
    /// the game intended.
    #[test]
    fn a_source_going_high_behind_another_raises_nothing() {
        let mut gpu = lcd_on(0x48, 0xFF); // LYC + HBlank selected
        gpu.wb(0xFF45, 5);
        advance_lines(&mut gpu, 4);
        assert_eq!(gpu.line, 4);

        // The window below covers the LY 4->5 increment (LYC coincidence goes
        // high) and then all of line 5's mode 2 -> 3 -> 0 walk.
        let raises = count_stat_raises(&mut gpu, DOTS_PER_LINE);
        assert_eq!(gpu.line, 5, "should have crossed into the LYC line");
        assert_eq!(
            raises, 1,
            "LYC and HBlank share one line: entering HBlank with the LYC \
             source already high must not raise a second interrupt"
        );

        // Control: the same line with only the HBlank source selected still
        // fires once, so the assertion above is not just counting nothing.
        let mut gpu = lcd_on(0x08, 0xFF);
        gpu.wb(0xFF45, 5);
        advance_lines(&mut gpu, 4);
        assert_eq!(count_stat_raises(&mut gpu, DOTS_PER_LINE), 1);
    }

    /// The `LY == LYC` bit in `STAT` is a flip-flop clocked by the PPU, not a
    /// live comparison. Switching the LCD off stops the comparison clock: the
    /// flag keeps its value, a new `LYC` cannot move it, and switching the LCD
    /// back on re-latches it against the `LY` the PPU restarts from. All three
    /// steps are read back by `acceptance/ppu/stat_lyc_onoff` round 1.
    #[test]
    fn the_lyc_comparison_clock_stops_with_the_lcd() {
        let mut gpu = lcd_on(0x40, 0xFF); // LYC source selected
        gpu.wb(0xFF45, 3);
        advance_lines(&mut gpu, 3);
        assert_eq!(gpu.line, 3);
        assert_eq!(gpu.rb(0xFF41) & 0x04, 0x04, "LY == LYC == 3");

        gpu.interrupt = 0;
        gpu.wb(0xFF40, 0x00); // LCD off
        assert_eq!(
            gpu.rb(0xFF41),
            0xC4,
            "the coincidence bit is retained, and mode reads 0"
        );
        assert_eq!(
            gpu.interrupt & 0x02,
            0,
            "the line never fell, so there is no edge to fire on"
        );

        gpu.wb(0xFF45, 0x01); // a new LYC while the clock is stopped
        assert_eq!(gpu.rb(0xFF41), 0xC4, "invisible until the PPU runs again");

        gpu.wb(0xFF40, 0x80); // LCD on: LY=0 vs LYC=1
        assert_eq!(gpu.rb(0xFF41) & 0x04, 0, "re-latched against LY=0");
        assert_eq!(gpu.interrupt & 0x02, 0, "a falling flag raises nothing");
    }

    /// ...and the same restart *can* raise a STAT interrupt when it flips the
    /// flag on — `stat_lyc_onoff` round 4 turns the PPU on with `LYC=0` and
    /// requires the interrupt, while round 2 does it with the flag already set
    /// and requires silence.
    #[test]
    fn enabling_the_lcd_raises_stat_only_if_the_coincidence_flag_flips_on() {
        // Round 4: flag false while off, true on restart -> one interrupt.
        let mut gpu = lcd_on(0x40, 0xFF);
        gpu.wb(0xFF45, 0x00);
        advance_lines(&mut gpu, 3); // LY=3 != LYC=0
        gpu.wb(0xFF40, 0x00);
        gpu.interrupt = 0;
        gpu.wb(0xFF40, 0x80);
        assert_eq!(gpu.interrupt & 0x02, 0x02, "LY=0 == LYC=0 is a rising edge");

        // Round 2: flag true while off, still true on restart -> silence.
        let mut gpu = lcd_on(0x40, 0xFF);
        gpu.wb(0xFF45, 3);
        advance_lines(&mut gpu, 3); // LY=3 == LYC=3, flag set
        gpu.wb(0xFF40, 0x00);
        gpu.wb(0xFF45, 0x00); // ignored while the clock is stopped
        gpu.interrupt = 0;
        gpu.wb(0xFF40, 0x80); // LY=0 == LYC=0: the flag does not change
        assert_eq!(gpu.interrupt & 0x02, 0, "no edge, no interrupt");
    }

    /// The scanline the PPU restarts on has **no OAM scan**: `STAT` reads
    /// mode 0 where every other line reads mode 2, and goes straight to
    /// mode 3. `acceptance/ppu/lcdon_timing-GS` states it outright ("line 0
    /// starts with mode 0 and goes straight to mode 3") and `stat_lyc_onoff`
    /// reads `STAT` back one instruction after the enable to check it.
    #[test]
    fn the_line_the_ppu_restarts_on_has_no_oam_scan() {
        let mut gpu = Gpu::new();
        gpu.wb(0xFF40, 0x80);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 0, "mode 0, not mode 2");
        gpu.do_cycle(76);
        assert_eq!(
            gpu.rb(0xFF41) & 0x03,
            0,
            "still mode 0 through the OAM slot"
        );
        gpu.do_cycle(4);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 3, "straight to pixel transfer");

        // The next line is an ordinary one again.
        gpu.do_cycle(DOTS_PER_LINE - 84 + 4);
        assert_eq!(gpu.line, 1);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 2, "line 1 has a normal OAM scan");
    }

    /// **One STAT interrupt per scanline, not two.** The OAM strobe is the
    /// first `OAM_DOTS` of a visible line, so it must already be asserted on
    /// the dot `LY` moves to that line. Asking `mode == 2` instead asks a
    /// field the dot loop moves *after* `next_line`, so the strobe dropped for
    /// the boundary dot and rose again — a second rising edge, i.e. a second
    /// interrupt, on every one of lines 1-143.
    ///
    /// It hid for a long time because the second edge lands inside the first
    /// one's dispatch, which clears `IF.1` at M4 after it: any program that
    /// services the interrupt immediately sees one. Line 0 had nothing to
    /// collapse into — by design it gets no early window — so there the defect
    /// was the whole error, and it showed up as `acceptance/ppu/intr_1_2_timing-GS`
    /// (the LY=0 mode-2 interrupt a full loop iteration late) and as line 0,
    /// and only line 0, coming out four pixels right on every Mealybug ROM
    /// that syncs off this source.
    #[test]
    fn a_visible_line_strobes_the_oam_source_exactly_once() {
        let mut gpu = lcd_on(0x20, 0xFF); // only the mode-2 source
        gpu.set_render_mode(RenderMode::Accurate);
        // Past the LCD-on restart line, which deliberately has no OAM scan.
        advance_lines(&mut gpu, 2);
        for line in 3..8 {
            assert_eq!(
                count_stat_raises(&mut gpu, DOTS_PER_LINE),
                1,
                "line {line} must raise the OAM source once, not once per \
                 boundary and once per mode change"
            );
        }
    }

    /// **Only the mode-2 strobe is asserted part-way through an M-cycle.**
    ///
    /// That is what confines the extra HALT latency
    /// ([`crate::cpu::core::Cpu::exec`]) to this one source. If the mode-0 or
    /// the line-144 strobe answered `true` here, `acceptance/ppu/
    /// hblank_ly_scx_timing-GS` and `ppu/intr_1_2_timing-GS` — which both wake
    /// from `HALT` on those sources — would each come out an M-cycle long.
    #[test]
    fn only_the_mode_2_strobe_is_asserted_inside_an_m_cycle() {
        // (STAT source bits, how many raises must be marked mid-M-cycle)
        for (stat, want_early) in [
            (0x20, true),  // mode 2 — the OAM strobe, asserted at dot 452
            (0x08, false), // mode 0 — flips on an M-cycle boundary
            (0x40, false), // LYC   — flips with `LY`, also a boundary
        ] {
            let mut gpu = lcd_on(stat, 0xFF);
            gpu.set_render_mode(RenderMode::Accurate);
            gpu.wb(0xFF45, 100); // LYC, for the third case
            advance_lines(&mut gpu, 2);

            let (mut early, mut boundary) = (0, 0);
            for _ in 0..(DOTS_PER_LINE * 140 / 4) {
                gpu.interrupt = 0;
                gpu.do_cycle(4);
                if gpu.interrupt & 0x02 != 0 {
                    if gpu.take_stat_irq_mid_mcycle() {
                        early += 1;
                    } else {
                        boundary += 1;
                    }
                }
            }
            assert!(early + boundary > 0, "source {stat:#04x} never fired");
            if want_early {
                assert_eq!(boundary, 0, "source {stat:#04x}: all mid-M-cycle");
            } else {
                assert_eq!(early, 0, "source {stat:#04x}: none mid-M-cycle");
            }
        }
    }

    /// The OAM strobe that the mode-2 select bit watches happens on line 144
    /// and **nowhere else in VBlank**. Holding it for all ten VBlank lines
    /// keeps the STAT line high across them, which blocks the real mode 2 at
    /// `LY=0` — `acceptance/ppu/intr_1_2_timing-GS` counts the gap between the
    /// two interrupts and sees it arrive a whole scanline late.
    #[test]
    fn the_vblank_oam_strobe_is_line_144_only() {
        let mut gpu = lcd_on(0x20, 0xFF); // only the mode-2 source
        advance_lines(&mut gpu, 143);
        assert_eq!(gpu.line, 143);

        gpu.interrupt = 0;
        gpu.do_cycle(DOTS_PER_LINE);
        assert_eq!(gpu.line, 144);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 1, "STAT reads mode 1 at LY=144");
        assert_eq!(gpu.interrupt & 0x02, 0x02, "...but the OAM source strobes");

        // Lines 145-153: nothing.
        assert_eq!(
            count_stat_raises(&mut gpu, DOTS_PER_LINE * 9),
            0,
            "the rest of VBlank has no OAM scan to strobe"
        );
        assert_eq!(gpu.line, 153);

        // And the first real OAM scan of the next frame is not blocked by it.
        assert_eq!(count_stat_raises(&mut gpu, DOTS_PER_LINE), 1);
        assert_eq!(gpu.line, 0);
    }

    /// Color hardware starts that strobe one M-cycle sooner than a DMG does.
    /// Mooneye ships the same program twice with one `nop` moved —
    /// `acceptance/ppu/vblank_stat_intr-GS` (DMG/MGB/SGB) and
    /// `misc/ppu/vblank_stat_intr-C` (CGB/AGB/AGS) — and each fails on the
    /// other machine, so the gap is a two-sided constraint: exactly one
    /// M-cycle on Color, exactly zero on a DMG.
    #[test]
    fn a_colour_console_strobes_one_m_cycle_before_the_vblank_flag() {
        for (mode, want_lead) in [
            (GbMode::Classic, false),
            (GbMode::Super, false),
            (GbMode::Color, true),
            (GbMode::ColorAsClassic, true),
        ] {
            let mut gpu = lcd_on(0x20, 0xFF); // only the mode-2 source
            gpu.gbmode = mode;
            advance_lines(&mut gpu, 143);
            // `lcd_on` restarts the PPU four dots into line 0, so line 143 is
            // reached at `modeclock == 4`; stop four dots short of its end.
            assert_eq!((gpu.line, gpu.modeclock), (143, 4), "{mode:?}");
            gpu.interrupt = 0;
            gpu.do_cycle(DOTS_PER_LINE - 8);
            assert_eq!(gpu.line, 143, "{mode:?}");
            assert_eq!(
                gpu.interrupt & 0x02 != 0,
                want_lead,
                "{mode:?}: STAT one M-cycle early"
            );
            assert_eq!(gpu.interrupt & 0x01, 0, "{mode:?}: VBlank is not early");

            // The remaining M-cycle brings VBlank, and the STAT line is
            // already high so there is no second edge.
            gpu.interrupt = 0;
            gpu.do_cycle(4);
            assert_eq!(gpu.line, 144, "{mode:?}");
            assert_eq!(gpu.interrupt & 0x01, 0x01, "{mode:?}: VBlank now");
            assert_eq!(
                gpu.interrupt & 0x02 != 0,
                !want_lead,
                "{mode:?}: one STAT edge in total, not two"
            );
        }
    }

    /// `set_boot_position` parks the PPU where a boot ROM left it, which is not
    /// the same thing as an `LCDC` write restarting it: the PPU has been
    /// free-running for frames, so the line it lands on has a normal OAM scan.
    #[test]
    fn a_booted_ppu_is_parked_mid_frame_and_not_restarted() {
        let mut gpu = lcd_on(0x00, 0xFF);
        gpu.set_boot_position(145 * DOTS_PER_LINE + 180);
        assert_eq!(gpu.line, 145);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 1, "VBlank");
        assert!(!gpu.skip_first_oam, "not an LCD-enable restart");

        // Somewhere in the middle of a visible line: mode follows the dot.
        gpu.set_boot_position(10 * DOTS_PER_LINE + 100);
        assert_eq!(gpu.line, 10);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 3, "dot 100 is pixel transfer");
    }

    /// Park the per-dot engine on `dot` of an ordinary visible line — past the
    /// LCD-on restart line, which deliberately has no OAM scan.
    fn at_dot(gpu: &mut Gpu, dot: u32) {
        advance_lines(gpu, 2);
        while gpu.line != 3 || gpu.modeclock != dot {
            gpu.do_cycle(1);
        }
    }

    /// **The mode-0 STAT interrupt is one dot ahead of the mode bits.** Mooneye
    /// measures the same interval twice — `intr_2_0_timing` from the mode-2
    /// interrupt to the mode-0 *interrupt*, `intr_2_mode0_timing` to the first
    /// `STAT` read that *reports* mode 0 — and the two answers differ by one
    /// M-cycle. Teeth: make [`Gpu::hblank_strobe`] plain `mode == 0` and the
    /// strobe stops leading, or apply the mode change at the bottom of the
    /// fetcher's last dot and the register stops lagging; either way this fails.
    #[test]
    fn the_hblank_interrupt_leads_the_mode_bits_by_one_dot() {
        let mut gpu = lcd_on(0x00, 0xFF);
        gpu.set_render_mode(RenderMode::Accurate);
        // `at_dot(d)` parks the PPU with dots `0..d` already run, which is where
        // a CPU access observes it. Dot 251 is the fetcher's last, so at 252 the
        // 160th pixel is out and the HBlank source is up — while the mode bits
        // still read 3, which is exactly what `lcdon_timing-GS` requires. They
        // only reach 0 once dot 252 has run.
        at_dot(&mut gpu, 251);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 3);
        assert!(!gpu.hblank_strobe(), "the fetcher has a dot left to run");

        at_dot(&mut gpu, 252);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 3, "the mode bits still report 3");
        assert!(
            gpu.hblank_strobe(),
            "...but the HBlank source is already up"
        );

        at_dot(&mut gpu, 253);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 0, "a dot later the bits follow");
        assert!(gpu.hblank_strobe());
    }

    /// **A read is blocked one dot before a write is**, at each dot the PPU
    /// takes a bus. `lcdon_timing-GS` and `lcdon_write_timing-GS` are the same
    /// nineteen offsets read and written, and they disagree at exactly these
    /// dots. Teeth: collapse either pair of predicates into one and this fails.
    #[test]
    fn a_write_still_lands_on_the_dot_a_read_is_already_blocked() {
        let mut gpu = lcd_on(0x00, 0xFF);
        gpu.set_render_mode(RenderMode::Accurate);

        // Dot 0: `LY` has moved and the OAM scan owns OAM, but `STAT` still
        // reads mode 0 for one more dot.
        at_dot(&mut gpu, 0);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 0);
        assert!(!gpu.cpu_can_read_oam(), "the scan already owns OAM");
        assert!(gpu.cpu_can_write_oam(), "...but a write still lands");
        assert!(gpu.cpu_can_read_vram(), "VRAM is free through the scan");

        // Dot 80: the fetcher owns VRAM, `STAT` still reads mode 2, and the
        // scan is over so an OAM write gets through again.
        at_dot(&mut gpu, 80);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 2);
        assert!(!gpu.cpu_can_read_vram(), "the fetcher already owns VRAM");
        assert!(gpu.cpu_can_write_vram(), "...but a write still lands");
        assert!(!gpu.cpu_can_read_oam());
        assert!(gpu.cpu_can_write_oam(), "the scan is over");

        // Mode 3 proper blocks both, both ways.
        at_dot(&mut gpu, 84);
        assert_eq!(gpu.rb(0xFF41) & 0x03, 3);
        for allowed in [
            gpu.cpu_can_read_vram(),
            gpu.cpu_can_write_vram(),
            gpu.cpu_can_read_oam(),
            gpu.cpu_can_write_oam(),
        ] {
            assert!(!allowed, "mode 3 blocks every access");
        }
    }

    /// **The coincidence comparator goes low while `LY` changes.** With
    /// `LYC = 3`, `lcdon_timing-GS` requires bit 2 *clear* at dot 0 of line 3
    /// and *set* at dot 4 — so the flag cannot simply follow `LY`. It is a
    /// blackout and not a hold: line 4 dot 0 reads clear too, with `LYC` still
    /// 3. Teeth: drop the `modeclock == 0` clause from [`Gpu::lyc_signal`].
    #[test]
    fn the_lyc_flag_blacks_out_while_ly_changes() {
        let mut gpu = lcd_on(0x00, 0xFF);
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.wb(0xFF45, 3); // LYC = 3

        at_dot(&mut gpu, 0);
        assert_eq!(gpu.line, 3);
        assert_eq!(gpu.rb(0xFF41) & 0x04, 0, "no match on the dot LY moves");
        at_dot(&mut gpu, 4);
        assert_eq!(gpu.rb(0xFF41) & 0x04, 0x04, "matched a dot later");

        // ...and it is released, not held: LY = 4 no longer matches.
        while gpu.line != 4 || gpu.modeclock != 0 {
            gpu.do_cycle(1);
        }
        assert_eq!(gpu.rb(0xFF41) & 0x04, 0);
    }

    /// The batched renderer must see none of the three rules above: it has no
    /// sub-instruction phase for a one-dot edge to live in, and its five
    /// baselines are the ones that must not move.
    #[test]
    fn the_batched_renderer_keeps_the_plain_mode_rules() {
        let mut gpu = lcd_on(0x00, 0xFF); // RenderMode::Fast by default
        assert_eq!(gpu.render_mode(), RenderMode::Fast);
        advance_lines(&mut gpu, 3);
        for _ in 0..DOTS_PER_LINE {
            gpu.do_cycle(1);
            assert!(gpu.cpu_can_read_vram());
            assert!(gpu.cpu_can_write_vram());
            assert!(gpu.cpu_can_read_oam());
            assert!(gpu.cpu_can_write_oam());
            assert_eq!(gpu.hblank_strobe(), gpu.mode == 0);
            assert_eq!(gpu.lyc_signal(), gpu.lyc_match);
        }
    }

    /// Serialise a PPU with its framebuffer blanked, so what comes back is
    /// every field *except* the picture.
    fn state_without_the_picture(gpu: &mut Gpu) -> Vec<u8> {
        let picture = std::mem::replace(&mut gpu.data, Box::new([0; HEIGHT * WIDTH * 4]));
        let mut w = crate::savestate::Writer::new();
        gpu.save_state(&mut w);
        gpu.data = picture;
        w.buf
    }

    /// **The whole claim behind [`Gpu::set_render_pixels`], made checkable.**
    ///
    /// Two PPUs, identical VRAM and registers, driven dot for dot through
    /// several frames with the window switched on part-way — the case that
    /// matters, because `draw_bg` advances the window's internal line counter
    /// as a side effect and that counter is the one piece of non-output state
    /// rendering touches. After every scanline, everything the machine can
    /// observe must be byte-identical.
    ///
    /// Teeth: delete the `advance_window_line()` call from `renderscan`'s skip
    /// path and this fails at the first line the window is on.
    #[test]
    fn skipping_pixels_changes_nothing_the_cartridge_can_see() {
        let mut drawn = Gpu::new();
        let mut skipped = Gpu::new();
        skipped.set_render_pixels(false);
        assert!(drawn.render_pixels() && !skipped.render_pixels());

        for gpu in [&mut drawn, &mut skipped] {
            // Tile data that is not uniform, so a missed fetch would show.
            for (i, byte) in gpu.vram[..0x1000].iter_mut().enumerate() {
                *byte = (i as u8).wrapping_mul(37).wrapping_add(11);
            }
            // Tile map, sprites, palettes.
            for (i, byte) in gpu.vram[0x1800..0x1C00].iter_mut().enumerate() {
                *byte = (i % 97) as u8;
            }
            for (i, byte) in gpu.voam.iter_mut().enumerate() {
                *byte = (i as u8).wrapping_mul(13);
            }
            gpu.wb(0xFF47, 0xE4);
            gpu.wb(0xFF48, 0x1B);
            gpu.wb(0xFF4B, 7); // WX: window at the left edge
            gpu.wb(0xFF4A, 40); // WY
            gpu.wb(0xFF42, 3); // SCY, so the fetch is not tile-aligned
            gpu.wb(0xFF43, 5); // SCX
                               // LCD on, background + sprites + window enabled.
            gpu.wb(0xFF40, 0xE3);
        }

        for line in 0..(154u32 * 3) {
            drawn.do_cycle(DOTS_PER_LINE);
            skipped.do_cycle(DOTS_PER_LINE);
            // Move the window mid-frame, which is what makes `wy_pos` matter.
            if line == 60 {
                drawn.wb(0xFF40, 0xC3); // window off
                skipped.wb(0xFF40, 0xC3);
            }
            if line == 200 {
                drawn.wb(0xFF40, 0xE3); // and on again
                skipped.wb(0xFF40, 0xE3);
            }
            assert_eq!(
                state_without_the_picture(&mut drawn),
                state_without_the_picture(&mut skipped),
                "line {line}: skipping pixels moved PPU state the cartridge can observe"
            );
            assert_eq!(drawn.interrupt, skipped.interrupt, "line {line}: interrupts");
        }

        // ...and it really was doing different work: the picture is not the same.
        assert_ne!(
            drawn.data.iter().filter(|b| **b != 0).count(),
            0,
            "the drawn PPU produced no picture, so this test proved nothing"
        );
        assert_ne!(
            &drawn.data[..], &skipped.data[..],
            "pixels were drawn either way — the skip did nothing"
        );
    }

    /// The per-dot engine produces pixels whatever the declaration says,
    /// because there the fetcher *is* the timing. Reporting otherwise would
    /// invite a caller to skip a cost it is still paying.
    #[test]
    fn the_per_dot_engine_always_produces_pixels() {
        let mut gpu = Gpu::new();
        gpu.set_render_pixels(false);
        assert!(!gpu.pixels_effective());
        gpu.set_render_mode(RenderMode::Accurate);
        assert!(
            gpu.pixels_effective(),
            "the fetcher drives mode 3's length, so its pixels are not optional"
        );
        gpu.set_render_pixels(true);
        assert!(gpu.pixels_effective());
    }
}
