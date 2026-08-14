//! The **accurate** rendering engine: the PPU's real pixel pipeline, stepped
//! one dot at a time.
//!
//! See [`docs/components/ppu.md`](../../docs/components/ppu.md) for the mode
//! cycle this sits inside, and [`docs/mealybug.md`](../../docs/mealybug.md) for
//! what it is measured against. The companion engine —
//! [`RenderMode::Fast`](super::RenderMode::Fast) — draws a whole scanline in one
//! go at the mode 3 → 0 transition; everything in this file exists because that
//! reads every register exactly once per line, and a program that writes `BGP`
//! and restores it twelve dots later is invisible to it by construction.
//!
//! ## The shape of mode 3
//!
//! Two things run concurrently, both clocked by the dot:
//!
//! * a **fetcher**, which reads one background (or window) tile every 6 dots —
//!   tile number, bitplane 0, bitplane 1, two dots each — and pushes its 8
//!   pixels into an 8-deep FIFO when the FIFO empties;
//! * the **output**, which shifts one pixel out of that FIFO per dot and paints
//!   it, mixing in whatever the sprite FIFO holds at the same position.
//!
//! The fetcher produces 8 pixels per 6 dots and the output consumes 8 per 8, so
//! the fetcher spends 2 dots per tile waiting for room — that idle is where a
//! sprite fetch gets to slip in, and it is why the sprite penalty is variable.
//!
//! At the head of the line the fetcher runs **twice** for the first tile and
//! the first result is thrown away, which is the 12 dots that make mode 3's
//! minimum 172 rather than 160 (Pan Docs,
//! [Rendering](https://gbdev.io/pandocs/Rendering.html): mode 3 is 172-289
//! dots). `SCX & 7` pixels are then discarded from the front, still costing a
//! dot each — that is the whole of the "mode 3 is longer when SCX is not a
//! multiple of 8" rule, and it falls out here instead of being a special case.
//!
//! ## Registers are read where hardware reads them
//!
//! This is the part that earns the rewrite. A **colour index** goes into the
//! FIFO, never a shade: `BGP` is applied when the pixel is shifted *out*, so a
//! mid-line palette write moves a boundary part-way along the scanline. The
//! tile-map select bits are read at the tile-number fetch, `LCDC.4` at each
//! bitplane fetch, and `SCY` at all three — the mixing that Mealybug's
//! `m3_lcdc_bg_map_change`, `m3_lcdc_tile_sel_change` and `m3_scy_change`
//! photograph is the direct consequence
//! ([mealybug's own PPU notes](https://github.com/mattcurrie/mealybug-tearoom-tests),
//! Pan Docs [Scrolling § mid-frame
//! behavior](https://gbdev.io/pandocs/Scrolling.html)).

use super::{GbMode, Gpu, HEIGHT, WIDTH};

/// One pixel waiting in a FIFO.
///
/// `color` is the raw 2-bit **index** the fetcher read out of VRAM, never a
/// shade. Applying the palette at the output stage instead of at fetch time is
/// the whole reason a mid-line `BGP` write moves a boundary along the line
/// rather than recolouring everything already drawn.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) struct Pixel {
    /// 0-3, straight out of the two bitplanes.
    pub color: u8,
    /// DMG OBJ: 0 = `OBP0`, 1 = `OBP1`. CGB: palette 0-7. BG on CGB: palette.
    pub palette: u8,
    /// OBJ attribute bit 7 ("draw behind BG colours 1-3"), or for a CGB BG
    /// pixel the tile attribute's own priority bit.
    pub prio: bool,
    /// CGB only: which VRAM bank the tile came from — kept so the pixel is
    /// self-describing, and the OAM index for OBJ-over-OBJ priority.
    pub oam: u8,
}

/// A sprite the OAM scan accepted for this line. Only `Y` and `X` are latched
/// during mode 2; the tile number and attributes are read during the sprite
/// fetch in mode 3, which is what lets `LCDC.2` change the sprite's height
/// after it has already been selected.
#[derive(Clone, Copy, Default, Debug)]
pub(super) struct ScanSprite {
    pub y: u8,
    pub x: u8,
    pub oam: u8,
}

/// How many dots a background/window tile fetch takes before its pixels can be
/// pushed: 2 for the tile number, 2 per bitplane.
const FETCH_DOTS: u8 = 6;
/// A sprite fetch is the same three reads and costs the same six dots — but it
/// suspends the output rather than running beside it.
const OBJ_FETCH_DOTS: u8 = 6;
/// `Pipeline::obj_tile_paid` when no background tile has paid its per-object
/// penalty yet. A real tile index is `(X + SCX) / 8`, at most 52.
const NO_TILE: u8 = 0xFF;

/// Everything the per-dot pipeline needs between one dot and the next. All of
/// it is machine state: a save taken mid-mode-3 has to resume the same picture,
/// so every field here is serialised (see [`Pipeline::save_state`]).
#[derive(Clone, Debug)]
pub(super) struct Pipeline {
    // --- mode 2, the OAM scan ---
    /// The (at most ten) sprites this line, in the order they will be fetched.
    scan: [ScanSprite; 10],
    nscan: u8,

    // --- the background / window fetcher ---
    /// Dots spent in the current tile fetch. `>= FETCH_DOTS` means the data is
    /// ready and the fetcher is waiting for room in the FIFO.
    fetch_t: u8,
    /// Which tile along the line is being fetched, counting from the left edge
    /// of the visible area (so `SCX / 8` is added for the background, and the
    /// window counts from its own left edge).
    fetch_x: u8,
    tile_num: u8,
    tile_attr: u8,
    lo: u8,
    hi: u8,
    /// The first fetch of a scanline is thrown away. See the module docs.
    first_fetch: bool,
    bg: [Pixel; 8],
    bg_head: u8,
    bg_len: u8,

    // --- sprites ---
    /// Aligned to the output position: index 0 is the pixel that goes out next.
    obj: [Pixel; 8],
    /// 0 = no sprite fetch in progress, else how many dots it has run.
    obj_t: u8,
    /// Stall dots still owed before the fetch's remaining reads may run — the
    /// per-tile half of Pan Docs' OBJ penalty. See [`Gpu::start_obj_fetch`].
    obj_wait: u8,
    /// The background tile (`(X + SCX) / 8`) whose per-tile OBJ penalty has
    /// already been paid on this line; `NO_TILE` while none has. Objects are
    /// fetched left to right, so remembering the last one is enough.
    obj_tile_paid: u8,
    /// Index into `scan` of the sprite being fetched.
    obj_i: u8,
    obj_tile: u8,
    obj_flags: u8,
    obj_lo: u8,
    obj_hi: u8,
    /// Bit *i* set once `scan[i]` has been fetched, so it is not fetched twice.
    fetched: u16,

    // --- output ---
    /// Pixels painted on this line so far; mode 3 ends when it reaches 160.
    lx: u8,
    /// `SCX & 7` pixels are shifted out and dropped at the head of the line.
    discard: u8,
    /// The fetcher is drawing the window rather than the background.
    window: bool,
    /// The window was drawn somewhere on this line — which is what advances the
    /// window's internal line counter.
    win_drawn: bool,
}

impl Default for Pipeline {
    fn default() -> Self {
        Pipeline {
            scan: [ScanSprite::default(); 10],
            nscan: 0,
            fetch_t: 0,
            fetch_x: 0,
            tile_num: 0,
            tile_attr: 0,
            lo: 0,
            hi: 0,
            first_fetch: true,
            bg: [Pixel::default(); 8],
            bg_head: 0,
            bg_len: 0,
            obj: [Pixel::default(); 8],
            obj_t: 0,
            obj_wait: 0,
            obj_tile_paid: NO_TILE,
            obj_i: 0,
            obj_tile: 0,
            obj_flags: 0,
            obj_lo: 0,
            obj_hi: 0,
            fetched: 0,
            lx: 0,
            discard: 0,
            window: false,
            win_drawn: false,
        }
    }
}

impl Pipeline {
    fn bg_push(&mut self, px: [Pixel; 8]) {
        for (i, p) in px.into_iter().enumerate() {
            self.bg[(self.bg_head as usize + i) & 7] = p;
        }
        self.bg_len = 8;
    }

    fn bg_pop(&mut self) -> Pixel {
        let p = self.bg[self.bg_head as usize & 7];
        self.bg_head = (self.bg_head + 1) & 7;
        self.bg_len -= 1;
        p
    }

    fn bg_clear(&mut self) {
        self.bg_len = 0;
        self.bg_head = 0;
    }

    /// Shift the sprite window one pixel left, refilling from the right with a
    /// transparent pixel. The sprite FIFO is always exactly eight entries wide
    /// and aligned to the output position, so a sprite merged at `lx` lands at
    /// index 0 and simply rides out with the background.
    fn obj_shift(&mut self) -> Pixel {
        let p = self.obj[0];
        self.obj.rotate_left(1);
        self.obj[7] = Pixel::default();
        p
    }

    pub(super) fn save_state(&self, w: &mut crate::savestate::Writer) {
        for s in &self.scan {
            w.u8(s.y);
            w.u8(s.x);
            w.u8(s.oam);
        }
        w.u8(self.nscan);
        w.u8(self.fetch_t);
        w.u8(self.fetch_x);
        w.u8(self.tile_num);
        w.u8(self.tile_attr);
        w.u8(self.lo);
        w.u8(self.hi);
        w.bool(self.first_fetch);
        Self::save_fifo(w, &self.bg);
        w.u8(self.bg_head);
        w.u8(self.bg_len);
        Self::save_fifo(w, &self.obj);
        w.u8(self.obj_t);
        w.u8(self.obj_wait);
        w.u8(self.obj_tile_paid);
        w.u8(self.obj_i);
        w.u8(self.obj_tile);
        w.u8(self.obj_flags);
        w.u8(self.obj_lo);
        w.u8(self.obj_hi);
        w.u16(self.fetched);
        w.u8(self.lx);
        w.u8(self.discard);
        w.bool(self.window);
        w.bool(self.win_drawn);
    }

    fn save_fifo(w: &mut crate::savestate::Writer, f: &[Pixel; 8]) {
        for p in f {
            w.u8(p.color);
            w.u8(p.palette);
            w.bool(p.prio);
            w.u8(p.oam);
        }
    }

    fn load_fifo(
        r: &mut crate::savestate::Reader,
        f: &mut [Pixel; 8],
    ) -> Result<(), crate::savestate::StateError> {
        for p in f.iter_mut() {
            p.color = r.u8()?;
            p.palette = r.u8()?;
            p.prio = r.bool()?;
            p.oam = r.u8()?;
        }
        Ok(())
    }

    pub(super) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        for s in self.scan.iter_mut() {
            s.y = r.u8()?;
            s.x = r.u8()?;
            s.oam = r.u8()?;
        }
        self.nscan = r.u8()?;
        self.fetch_t = r.u8()?;
        self.fetch_x = r.u8()?;
        self.tile_num = r.u8()?;
        self.tile_attr = r.u8()?;
        self.lo = r.u8()?;
        self.hi = r.u8()?;
        self.first_fetch = r.bool()?;
        Self::load_fifo(r, &mut self.bg)?;
        self.bg_head = r.u8()?;
        self.bg_len = r.u8()?;
        Self::load_fifo(r, &mut self.obj)?;
        self.obj_t = r.u8()?;
        self.obj_wait = r.u8()?;
        self.obj_tile_paid = r.u8()?;
        self.obj_i = r.u8()?;
        self.obj_tile = r.u8()?;
        self.obj_flags = r.u8()?;
        self.obj_lo = r.u8()?;
        self.obj_hi = r.u8()?;
        self.fetched = r.u16()?;
        self.lx = r.u8()?;
        self.discard = r.u8()?;
        self.window = r.bool()?;
        self.win_drawn = r.bool()?;
        Ok(())
    }
}

impl Gpu {
    /// Advance the accurate engine by `ticks` dots.
    ///
    /// Modes 2 and 3 are stepped **one dot at a time** — that is the whole
    /// point — while modes 0 and 1 are jumped through in one go, because
    /// nothing inside them is sub-dot observable and they are two thirds of the
    /// frame. The caller (`MemoryManagementUnit::bus_tick`) hands us one
    /// M-cycle per CPU memory access, so a register write lands on exactly the
    /// dot the program put it on.
    pub(super) fn do_cycle_accurate(&mut self, ticks: u32) {
        let mut left = ticks;
        while left > 0 {
            // Modes 0 and 1 have no per-dot behaviour: skip straight to the end
            // of the scanline (or as far as the caller's budget reaches).
            //
            // The `modeclock` half of the test is load-bearing: `STAT` also
            // reads mode 0 at the *head* of a visible line — both in the dot
            // before mode 2 is entered and for the whole of the OAM slot on the
            // line the PPU restarts on after `LCDC.7` goes 0→1 — and skipping
            // there would jump the scanline the pipeline is about to draw.
            //
            // `skip_first_oam` has to be excluded outright, and it is not a
            // refinement: the line the PPU restarts on reads mode 0 *through*
            // its OAM slot and only leaves it by entering mode 3 at dot 80,
            // which is the one place that clears the flag. Let this shortcut
            // fire there and the PPU never reaches dot 80 on that line, so the
            // flag survives to the next line, and the next — the whole engine
            // stays parked in mode 0 with `LY` ticking over and **no mode 2, no
            // mode 3 and no STAT interrupt for the rest of the frame**. Every
            // Mealybug ROM arms itself off the mode-2 STAT source, so they all
            // ran their nop-slide off the end into a crash instead. See
            // `the_ppu_restarts_into_mode_3_after_the_lcd_comes_on`.
            let idle = self.line >= VBLANK
                || (self.mode == 0 && self.modeclock >= OAM_DOTS && !self.skip_first_oam);
            if idle {
                let to_eol = LINE_DOTS - self.modeclock;
                let step = left.min(to_eol);
                self.modeclock += step;
                left -= step;
                if self.modeclock >= LINE_DOTS {
                    self.next_line();
                }
                self.refresh_stat_line();
                continue;
            }
            self.accurate_dot();
            left -= 1;
        }
    }

    /// One dot of a visible scanline in mode 2 or mode 3.
    ///
    /// `modeclock` is the **0-based index of the dot about to be run**, so the
    /// work happens first and the counter moves afterwards: dot 0 is the first
    /// dot of mode 2 and dot 80 the first of mode 3, which is how Pan Docs
    /// numbers them and how every figure in this file is stated. Incrementing
    /// first made every mode boundary land a dot later than its documented
    /// number, so `start_mode3` and `oam_scan_dot` each carried a compensating
    /// `- 1`. It moved no test either way — it is a change of units, not of
    /// behaviour — but every dot count here can now be read against the
    /// documentation without a mental offset.
    fn accurate_dot(&mut self) {
        if self.modeclock < OAM_DOTS {
            // The line the PPU restarts on after `LCDC` bit 7 goes 0→1 has no
            // OAM scan at all: `STAT` reads mode 0 and the mode-2 source does
            // not strobe (`acceptance/ppu/lcdon_timing-GS`).
            if self.skip_first_oam {
                self.check_wy_trigger();
            } else {
                if self.mode != 2 {
                    self.start_oam_scan();
                    self.change_mode(2);
                }
                self.oam_scan_dot();
            }
            if self.modeclock + 1 == OAM_DOTS {
                // Mode 3 begins on the *next* dot; set the pipeline up now so
                // its first fetch happens on that dot rather than a dot late.
                self.start_mode3();
            }
        } else {
            if self.mode != 3 {
                self.skip_first_oam = false;
                self.change_mode(3);
            }
            self.pipeline_dot();
            if self.pipe.lx as usize >= WIDTH {
                self.change_mode(0);
            }
        }

        self.modeclock += 1;
        if self.modeclock >= LINE_DOTS {
            self.next_line();
        }
        self.refresh_stat_line();
    }

    /// Cross into the next scanline: `LY` moves, the coincidence flag
    /// re-latches, and line 144 starts VBlank.
    fn next_line(&mut self) {
        self.modeclock -= LINE_DOTS;
        self.line = (self.line + 1) % 154;
        self.refresh_lyc_match();
        if self.line >= VBLANK && self.mode != 1 {
            self.change_mode(1);
        }
    }

    // ---------------------------------------------------------------- mode 2

    fn start_oam_scan(&mut self) {
        self.pipe.nscan = 0;
        self.pipe.fetched = 0;
    }

    /// One dot of the OAM scan. Hardware checks one sprite every two dots, so
    /// forty of them fill the 80-dot mode; `LCDC.2` is read live, which is what
    /// `m3_lcdc_obj_size_change` changes underneath it.
    fn oam_scan_dot(&mut self) {
        // Dots 0..79 -> entries 0..39, one per two dots.
        if self.modeclock % 2 != 0 {
            return;
        }
        let index = (self.modeclock / 2) as usize;
        if index >= 40 || self.pipe.nscan >= 10 {
            return;
        }
        let base = index * 4;
        let y = self.voam[base] as i32 - 16;
        let line = self.line as i32;
        if line < y || line >= y + self.sprite_size as i32 {
            return;
        }
        let n = self.pipe.nscan as usize;
        self.pipe.scan[n] = ScanSprite {
            y: self.voam[base],
            x: self.voam[base + 1],
            oam: index as u8,
        };
        self.pipe.nscan += 1;
    }

    // ---------------------------------------------------------------- mode 3

    /// Arm the pipeline for a fresh scanline.
    fn start_mode3(&mut self) {
        let p = &mut self.pipe;
        p.fetch_t = 0;
        p.fetch_x = 0;
        p.first_fetch = true;
        p.bg_clear();
        p.obj = [Pixel::default(); 8];
        p.obj_t = 0;
        p.obj_wait = 0;
        p.obj_tile_paid = NO_TILE;
        p.lx = 0;
        p.window = false;
        p.win_drawn = false;
        p.fetched = 0;
        // The low three bits of SCX are latched here and nowhere else: they are
        // read once at the start of the scanline, unlike the high five which
        // are re-read at every tile fetch (Pan Docs, Scrolling § mid-frame
        // behavior). `m3_scx_low_3_bits` and `m3_scx_high_5_bits` are exactly
        // this distinction, one register split into two behaviours.
        p.discard = self.scx & 7;
        // DMG sprite priority is by X first, OAM index second, and hardware
        // gets it for free by fetching sprites in the order the output reaches
        // them. Sorting the scan list the same way, and letting a merge write
        // only where the sprite FIFO is still transparent, reproduces it. A
        // Color console ignores X and goes purely by OAM index, which is the
        // order the scan already produced.
        if self.gbmode != GbMode::Color {
            let n = self.pipe.nscan as usize;
            self.pipe.scan[..n].sort_by_key(|s| (s.x, s.oam));
        }
    }

    /// One dot of the pixel pipeline.
    ///
    /// Order matters and is not arbitrary: a sprite fetch freezes both the
    /// output and the background fetcher, the output shifts a pixel that was
    /// already in the FIFO *before* this dot's fetch could push, and the
    /// fetcher then gets its dot. That ordering is what makes the first pixel
    /// appear on dot 12 and mode 3 come out at 172 dots for `SCX & 7 == 0`.
    fn pipeline_dot(&mut self) {
        self.check_window_start();

        // A sprite fetch in progress owns the dot outright.
        if self.pipe.obj_t > 0 {
            self.obj_fetch_dot();
            return;
        }

        // A sprite is due here: the output stops until it has been fetched.
        if let Some(i) = self.due_sprite() {
            if self.pipe.bg_len > 0 {
                self.start_obj_fetch(i);
                return;
            }
        }

        self.output_dot();
        self.bg_fetch_dot();
    }

    /// The window takes over the fetcher the moment the output reaches `WX-7`,
    /// provided the frame-long `WY` condition has already latched.
    ///
    /// Re-enabling `LCDC.5` later on the same line starts the window's **next**
    /// row, which is why the line counter is advanced here — at every
    /// activation — rather than once per scanline. Mealybug's
    /// `m3_lcdc_win_en_change_multiple` is built entirely out of that.
    fn check_window_start(&mut self) {
        if self.pipe.window || !self.win_on || !self.wy_trigger {
            return;
        }
        // The comparison is made **at the output**, against the pixel about to
        // be shifted out — so it cannot happen before there is one. Checking it
        // during the head-of-line fetch, where `lx` is already 0 and the FIFO is
        // still empty, let a window with `WX <= 7` take over a fetcher that had
        // nothing to throw away and so cost nothing: `m3_window_timing` sets
        // `WX = LY` on every line and photographs the difference between a
        // window that started before the palette write and one that started
        // after, which is exactly the six dots of the window's own startup
        // fetch. Without this the boundary sat at the same pixel for every `WX`.
        if self.pipe.bg_len == 0 {
            return;
        }
        if self.winx > 166 {
            return;
        }
        let start = (self.winx as i16) - 7;
        if (self.pipe.lx as i16) < start {
            return;
        }
        self.wy_pos += 1;
        let p = &mut self.pipe;
        p.window = true;
        p.win_drawn = true;
        p.fetch_x = 0;
        p.fetch_t = 0;
        p.bg_clear();
    }

    /// Turning `LCDC.5` off mid-line does not take effect until the window tile
    /// being drawn has finished; the background then resumes **on a tile
    /// boundary**, with the low three bits of `SCX` having no further effect
    /// (mealybug's own PPU notes, `WIN_EN`).
    fn check_window_stop(&mut self) {
        if !self.pipe.window || self.win_on {
            return;
        }
        let resume = (self.pipe.lx / 8) + 1;
        let p = &mut self.pipe;
        p.window = false;
        p.fetch_x = resume;
        p.fetch_t = 0;
        p.bg_clear();
    }

    /// Advance the background/window fetcher one dot, reading each register at
    /// the stage hardware reads it at.
    fn bg_fetch_dot(&mut self) {
        if self.pipe.fetch_t < FETCH_DOTS {
            self.pipe.fetch_t += 1;
            match self.pipe.fetch_t {
                2 => self.fetch_tile_number(),
                4 => self.lo_hi(false),
                6 => self.lo_hi(true),
                _ => {}
            }
        }
        if self.pipe.fetch_t < FETCH_DOTS {
            return;
        }
        // The first fetch of a scanline is discarded: the fetcher immediately
        // starts the same tile again. Those six dots are why mode 3 is 172 dots
        // long and not 166.
        if self.pipe.first_fetch {
            self.pipe.first_fetch = false;
            self.pipe.fetch_t = 0;
            return;
        }
        if self.pipe.bg_len > 0 {
            return; // no room yet — this is the fetcher's idle
        }
        self.push_bg_tile();
        self.pipe.fetch_x = self.pipe.fetch_x.wrapping_add(1);
        self.pipe.fetch_t = 0;
        self.check_window_stop();
    }

    fn fetch_tile_number(&mut self) {
        let (map, tx, ty) = if self.pipe.window {
            let wy = self.wy_pos.max(0) as u16;
            (
                self.win_tilemap,
                self.pipe.fetch_x as u16 & 31,
                (wy >> 3) & 31,
            )
        } else {
            let bgy = self.scy.wrapping_add(self.line) as u16;
            (
                self.bg_tilemap,
                ((self.scx as u16 >> 3) + self.pipe.fetch_x as u16) & 31,
                (bgy >> 3) & 31,
            )
        };
        let addr = map + ty * 32 + tx;
        self.pipe.tile_num = self.rbvram0(addr);
        self.pipe.tile_attr = if self.gbmode == GbMode::Color {
            self.rbvram1(addr)
        } else {
            0
        };
    }

    /// Read one bitplane. `LCDC.4` and `SCY` are both re-read here rather than
    /// latched at the tile-number fetch, which is what lets a one-M-cycle
    /// change to either mix data from two different tiles or two different rows
    /// into a single drawn tile.
    fn lo_hi(&mut self, high: bool) {
        let row = if self.pipe.window {
            (self.wy_pos.max(0) as u16) & 7
        } else {
            self.scy.wrapping_add(self.line) as u16 & 7
        };
        let flip_y = self.gbmode == GbMode::Color && self.pipe.tile_attr & 0x40 != 0;
        let row = if flip_y { 7 - row } else { row };
        let base = self.tilebase;
        let index = if base == 0x8000 {
            self.pipe.tile_num as u16
        } else {
            (self.pipe.tile_num as i8 as i16 + 128) as u16
        };
        let addr = base + index * 16 + row * 2 + u16::from(high);
        let bank1 = self.gbmode == GbMode::Color && self.pipe.tile_attr & 0x08 != 0;
        let byte = if bank1 {
            self.rbvram1(addr)
        } else {
            self.rbvram0(addr)
        };
        if high {
            self.pipe.hi = byte;
        } else {
            self.pipe.lo = byte;
        }
    }

    fn push_bg_tile(&mut self) {
        let (lo, hi) = (self.pipe.lo, self.pipe.hi);
        let attr = self.pipe.tile_attr;
        let cgb = self.gbmode == GbMode::Color;
        let flip_x = cgb && attr & 0x20 != 0;
        let mut px = [Pixel::default(); 8];
        for (i, slot) in px.iter_mut().enumerate() {
            let bit = if flip_x { i } else { 7 - i } as u32;
            let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            *slot = Pixel {
                color,
                palette: attr & 0x07,
                prio: cgb && attr & 0x80 != 0,
                oam: 0,
            };
        }
        self.pipe.bg_push(px);
    }

    // --------------------------------------------------------------- sprites

    /// The next sprite whose eight pixels start at, or have already reached,
    /// the current output position and which has not been fetched yet.
    fn due_sprite(&self) -> Option<usize> {
        if !self.sprite_on {
            return None;
        }
        let lx = self.pipe.lx as i16;
        (0..self.pipe.nscan as usize).find(|&i| {
            if self.pipe.fetched & (1 << i) != 0 {
                return false;
            }
            let x = self.pipe.scan[i].x as i16;
            // `x <= lx + 8` and nothing else. An object whose `X` is below 8
            // hangs off the left edge and shows no pixels at all, but hardware
            // still fetches it and still pays for it — Mooneye's
            // `intr_2_mode0_timing_sprites` opens with ten arrangements at
            // `X = 0` precisely to measure that, and its table says each one
            // costs its full six dots. Excluding them (`x > lx`, as this used
            // to) made every object at `X = 0` free.
            x <= lx + 8
        })
    }

    /// Begin an object fetch, charging **Pan Docs' OBJ penalty algorithm**
    /// ([Rendering § OBJ penalty algorithm](https://gbdev.io/pandocs/Rendering.html)):
    /// six dots for the fetch itself, plus — for the *first* object on each
    /// background tile — `max(0, 5 - ((X + SCX) % 8))` dots of stall while the
    /// tile the object lands on is finished.
    ///
    /// The formula is charged rather than emerging from the fetcher's phase,
    /// and Mooneye's `intr_2_mode0_timing_sprites` is why: its 39 arrangements
    /// give the answer for every object count and every position within a tile,
    /// and the phase this fetcher happens to be in at the head of a line
    /// reproduced only the maximum — ten objects at `X = 0` and ten at `X = 7`
    /// both came out at 66 dots where hardware wants 65 and 60. See
    /// `the_obj_penalty_matches_mooneyes_table`.
    fn start_obj_fetch(&mut self, i: usize) {
        let x = self.pipe.scan[i].x as i16 + self.scx as i16;
        let tile = (x >> 3) as u8;
        let penalty = if tile == self.pipe.obj_tile_paid {
            0
        } else {
            self.pipe.obj_tile_paid = tile;
            (5 - (x & 7)).max(0) as u8
        };
        self.pipe.obj_i = i as u8;
        self.pipe.fetched |= 1 << i;
        self.pipe.obj_wait = penalty;
        self.pipe.obj_t = 1;
        self.read_obj_attributes();
    }

    fn read_obj_attributes(&mut self) {
        let oam = self.pipe.scan[self.pipe.obj_i as usize].oam as usize * 4;
        self.pipe.obj_tile = self.voam[oam + 2];
        self.pipe.obj_flags = self.voam[oam + 3];
    }

    /// One dot of a sprite fetch. Six dots, the same three reads as a
    /// background tile, but the output is frozen throughout — that is the fixed
    /// half of the OBJ penalty (Pan Docs, "OBJ penalty algorithm").
    fn obj_fetch_dot(&mut self) {
        if self.pipe.obj_wait > 0 {
            self.pipe.obj_wait -= 1;
            return;
        }
        self.pipe.obj_t += 1;
        match self.pipe.obj_t {
            3 => self.obj_plane(false),
            5 => self.obj_plane(true),
            _ => {}
        }
        if self.pipe.obj_t >= OBJ_FETCH_DOTS {
            self.merge_sprite();
            self.pipe.obj_t = 0;
        }
    }

    fn obj_plane(&mut self, high: bool) {
        let s = self.pipe.scan[self.pipe.obj_i as usize];
        let flags = self.pipe.obj_flags;
        let tall = self.sprite_size == 16;
        let height = if tall { 16 } else { 8 };
        let mut row = self.line as i32 - (s.y as i32 - 16);
        if flags & 0x40 != 0 {
            row = height - 1 - row;
        }
        let row = row.clamp(0, height - 1) as u16;
        let tile = if tall {
            self.pipe.obj_tile & 0xFE
        } else {
            self.pipe.obj_tile
        } as u16;
        let addr = 0x8000 + tile * 16 + row * 2 + u16::from(high);
        let bank1 = self.gbmode == GbMode::Color && flags & 0x08 != 0;
        let byte = if bank1 {
            self.rbvram1(addr)
        } else {
            self.rbvram0(addr)
        };
        if high {
            self.pipe.obj_hi = byte;
        } else {
            self.pipe.obj_lo = byte;
        }
    }

    /// Merge the fetched sprite into the eight-pixel sprite window.
    ///
    /// A pixel is written only where the window is still transparent — that
    /// single rule *is* sprite-over-sprite priority on a DMG, because the scan
    /// list was sorted into the order hardware reaches them. A Color console
    /// resolves ties by OAM index instead, so there a lower index may overwrite
    /// a higher one.
    fn merge_sprite(&mut self) {
        let s = self.pipe.scan[self.pipe.obj_i as usize];
        let flags = self.pipe.obj_flags;
        let (lo, hi) = (self.pipe.obj_lo, self.pipe.obj_hi);
        let flip_x = flags & 0x20 != 0;
        let cgb = self.gbmode == GbMode::Color;
        // Where this sprite's leftmost pixel sits relative to the output.
        let start = s.x as i16 - 8 - self.pipe.lx as i16;
        for i in 0..8i16 {
            let slot = start + i;
            if !(0..8).contains(&slot) {
                continue;
            }
            let bit = if flip_x { i } else { 7 - i } as u32;
            let color = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
            if color == 0 {
                continue;
            }
            let existing = self.pipe.obj[slot as usize];
            let win = existing.color == 0 || (cgb && s.oam < existing.oam);
            if !win {
                continue;
            }
            self.pipe.obj[slot as usize] = Pixel {
                color,
                palette: if cgb {
                    flags & 0x07
                } else {
                    u8::from(flags & 0x10 != 0)
                },
                prio: flags & 0x80 != 0,
                oam: s.oam,
            };
        }
    }

    // ---------------------------------------------------------------- output

    /// Shift one pixel out of the FIFOs and paint it.
    fn output_dot(&mut self) {
        if self.pipe.bg_len == 0 {
            return;
        }
        let bg = self.pipe.bg_pop();
        let obj = self.pipe.obj_shift();
        if self.pipe.discard > 0 {
            self.pipe.discard -= 1;
            return;
        }
        let x = self.pipe.lx as usize;
        self.pipe.lx += 1;
        if x >= WIDTH || self.line as usize >= HEIGHT {
            return;
        }
        // STOP mode: the PPU has lost its VRAM access, so every pixel is the
        // same flat shade. See `Gpu::set_stop_fill`.
        if let Some(shade) = self.stop_fill {
            self.setcolor(x, shade);
            return;
        }
        self.paint(x, bg, obj);
    }

    fn paint(&mut self, x: usize, bg: Pixel, obj: Pixel) {
        if self.gbmode == GbMode::Color {
            self.paint_color(x, bg, obj);
            return;
        }
        // DMG: `LCDC.0` clear blanks the background and window. It is read
        // here, at the output, so a mid-line change moves the boundary — which
        // is what `m3_lcdc_bg_en_change` photographs.
        let bg_color = if self.lcdc0 { bg.color } else { 0 };
        let show_obj = obj.color != 0 && (!obj.prio || bg_color == 0);
        if show_obj {
            let pal = if obj.palette == 0 {
                self.pal0
            } else {
                self.pal1
            };
            self.setcolor(x, pal[obj.color as usize]);
        } else {
            self.setcolor(x, self.palb[bg_color as usize]);
        }
    }

    fn paint_color(&mut self, x: usize, bg: Pixel, obj: Pixel) {
        // On a Color console `LCDC.0` is the "BG and window master priority"
        // bit: clear, sprites win over every background pixel.
        let bg_wins = self.lcdc0 && (bg.prio || obj.prio) && bg.color != 0;
        if obj.color != 0 && !bg_wins {
            let c = self.csprit[obj.palette as usize][obj.color as usize];
            self.setrgb(x, c[0], c[1], c[2]);
        } else {
            let c = self.cbgpal[bg.palette as usize][bg.color as usize];
            self.setrgb(x, c[0], c[1], c[2]);
        }
    }
}

/// Dots of OAM scan at the head of every visible scanline.
const OAM_DOTS: u32 = super::OAM_DOTS;
const LINE_DOTS: u32 = super::LINE_DOTS;
const VBLANK: u8 = super::VBLANK_LINE;

/// Mode 3's length is the engine's headline observable — it is what the
/// mid-scanline Mooneye tests measure directly and what every Mealybug ROM
/// depends on to land its write on the right dot. The fast renderer answers a
/// flat 172 to all of these; nothing below would be true of it.
///
/// Figures are Pan Docs, [Rendering § mode 3
/// length](https://gbdev.io/pandocs/Rendering.html) and the "OBJ penalty
/// algorithm" section beside it.
#[cfg(test)]
mod mode3_length {
    use super::super::{Gpu, RenderMode};

    /// A DMG with the LCD on, the accurate engine selected, parked at the top
    /// of a fresh scanline.
    fn engine() -> Gpu {
        let mut gpu = Gpu::new();
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.wb(0xFF40, 0x91); // LCD on, BG on, tile data $8000
                              // The line the LCD comes up on has no OAM scan; step past it so every
                              // measurement is of an ordinary scanline.
        gpu.do_cycle(super::LINE_DOTS);
        gpu
    }

    /// Dots spent in mode 3 on the next scanline, measured one dot at a time.
    ///
    /// Counted off `modeclock` rather than by tallying steps: the dot that
    /// paints pixel 160 is itself a mode-3 dot *and* the one that flips STAT to
    /// mode 0, so a step-tallying loop is short by exactly one.
    fn mode3_dots(gpu: &mut Gpu) -> u32 {
        while gpu.mode() != 2 {
            gpu.do_cycle(1);
        }
        while gpu.mode() != 3 {
            gpu.do_cycle(1);
        }
        while gpu.mode() == 3 {
            gpu.do_cycle(1);
        }
        gpu.modeclock - super::OAM_DOTS
    }

    /// The floor: 160 pixels plus the 12 dots of the discarded first fetch.
    #[test]
    fn a_plain_background_line_is_172_dots() {
        let mut gpu = engine();
        assert_eq!(mode3_dots(&mut gpu), 172);
    }

    /// `SCX & 7` pixels are shifted out and thrown away at the head of the
    /// line, and each still costs its dot. This is the whole of the
    /// "mode 3 is longer when SCX is not a multiple of 8" rule, and it is a
    /// *consequence* here rather than a special case.
    #[test]
    fn scx_and_7_adds_one_dot_per_discarded_pixel() {
        for scx in 0..16u8 {
            let mut gpu = engine();
            gpu.wb(0xFF43, scx);
            assert_eq!(
                mode3_dots(&mut gpu),
                172 + (scx & 7) as u32,
                "SCX={scx}: only the low three bits may lengthen mode 3"
            );
        }
    }

    /// Only `SCX`'s low three bits are latched for the line. The high five
    /// select which tile the fetcher starts from and cost nothing — Mealybug
    /// splits exactly this one register into `m3_scx_low_3_bits` and
    /// `m3_scx_high_5_bits`.
    #[test]
    fn the_high_bits_of_scx_cost_nothing() {
        let mut gpu = engine();
        gpu.wb(0xFF43, 0xF8);
        assert_eq!(mode3_dots(&mut gpu), 172);
    }

    /// Put `n` sprites on the line the engine is parked in front of, all at the
    /// same `X`, and turn objects on.
    fn with_sprites(gpu: &mut Gpu, n: usize, xs: &[u8]) {
        gpu.wb(0xFF40, 0x93); // + LCDC.1: objects on
        for i in 0..40u16 {
            // Y = 0 puts a sprite above the screen, i.e. out of the scan.
            gpu.wb(0xFE00 + i * 4, 0);
            gpu.wb(0xFE00 + i * 4 + 1, 0);
        }
        // `engine()` parks a few dots into the OAM scan, which has therefore
        // already walked the first entries; start at 10 so every sprite here is
        // still ahead of it.
        for (i, x) in xs.iter().copied().take(n).enumerate() {
            let base = 0xFE00 + (i as u16 + 10) * 4;
            // `mode3_dots` measures the line the engine is parked at the top
            // of, so Y = LY + 16 puts the object on exactly that line.
            gpu.wb(base, 16 + gpu.rb(0xFF44));
            gpu.wb(base + 1, x);
        }
    }

    /// **Pan Docs' OBJ penalty algorithm, against Mooneye's own table.**
    ///
    /// `acceptance/ppu/intr_2_mode0_timing_sprites` measures the mode-2 →
    /// mode-0 distance for 39 sprite arrangements and states the answer for
    /// each in M-cycles above the base. Reading its numbers back as dots gives
    /// exactly `6` dots per object plus, for the **first** object on each
    /// background tile, `max(0, 5 - ((X + SCX) % 8))` — Pan Docs, [Rendering §
    /// OBJ penalty algorithm](https://gbdev.io/pandocs/Rendering.html).
    ///
    /// Worked from the ROM's own table (its unit is one M-cycle and it
    /// truncates, so a dot figure of 65 reads as 16):
    ///
    /// | arrangement | this formula | the ROM says |
    /// |---|---:|---:|
    /// | 1 sprite at X=0 | 6+5 = 11 → 2 | 2 |
    /// | 10 sprites at X=0 | 60+5 = 65 → 16 | 16 |
    /// | 10 at X=2 | 60+3 = 63 → 15 | 15 |
    /// | 10 at X=5..7 | 60+0 = 60 → 15 | 15 |
    /// | 5 at X=0 and 5 at X=160 | 60+5+5 = 70 → 17 | 17 |
    /// | 5 at X=4 and 5 at X=164 | 60+1+1 = 62 → 15 | 15 |
    /// **The window costs six dots wherever it starts, including at the very
    /// left edge.** Pan Docs, [Rendering § mode 3
    /// length](https://gbdev.io/pandocs/Rendering.html): "if the window is
    /// enabled and the current scanline reaches it, mode 3 is 6 dots longer".
    ///
    /// `WX <= 7` used to be free, because the window comparison ran during the
    /// head-of-line fetch — `lx` is already 0 there and the FIFO is still empty,
    /// so the takeover had nothing to throw away. Mealybug's `m3_window_timing`
    /// is built entirely out of the difference between a window that started
    /// before a mid-line `BGP` write and one that started after, and with the
    /// startup free the boundary sat at the same pixel for every `WX`.
    #[test]
    fn the_window_costs_six_dots_wherever_it_starts() {
        for wx in [0u8, 1, 7, 8, 20, 80, 166] {
            let mut gpu = engine();
            gpu.wb(0xFF4A, 0); // WY = 0, so the Y condition latches at once
            gpu.wb(0xFF4B, wx);
            gpu.wb(0xFF40, 0xB1); // + LCDC.5
            assert_eq!(mode3_dots(&mut gpu), 178, "WX={wx}");
        }
        // `WX > 166` puts the window entirely off the right edge, so it never
        // takes over and costs nothing.
        let mut gpu = engine();
        gpu.wb(0xFF4A, 0);
        gpu.wb(0xFF4B, 167);
        gpu.wb(0xFF40, 0xB1);
        assert_eq!(mode3_dots(&mut gpu), 172);
    }

    #[test]
    fn the_obj_penalty_matches_mooneyes_table() {
        // n sprites all on one background tile: one tile penalty, n fetches.
        for n in 1..=10usize {
            let mut gpu = engine();
            with_sprites(&mut gpu, n, &[0u8; 10]);
            assert_eq!(
                mode3_dots(&mut gpu),
                172 + 6 * n as u32 + 5,
                "{n} sprites at X=0"
            );
        }
        // One tile, ten sprites, sweeping where in the tile they sit.
        for x in 0..8u8 {
            let mut gpu = engine();
            with_sprites(&mut gpu, 10, &[x; 10]);
            assert_eq!(
                mode3_dots(&mut gpu),
                172 + 60 + 5u32.saturating_sub(x as u32),
                "ten sprites at X={x}"
            );
        }
        // Two groups a whole number of tiles apart: each group's first object
        // pays its own tile penalty.
        for x in 0..6u8 {
            let mut gpu = engine();
            let xs = [x, x, x, x, x, 160 + x, 160 + x, 160 + x, 160 + x, 160 + x];
            with_sprites(&mut gpu, 10, &xs);
            let one = 5u32.saturating_sub(x as u32);
            assert_eq!(
                mode3_dots(&mut gpu),
                172 + 60 + 2 * one,
                "five sprites at X={x} and five at X={}",
                160 + x
            );
        }
    }
}

/// What the PPU does on the line the LCD comes up on.
///
/// This is its own module because the answer used to be *nothing*, for the
/// whole rest of the frame, and every Mealybug ROM died of it — see
/// [`docs/mealybug.md`](../../docs/mealybug.md) §7.6.
#[cfg(test)]
mod lcd_restart {
    use super::super::{Gpu, RenderMode};

    /// A DMG with the accurate engine selected and `LCDC.7` just taken 0→1,
    /// i.e. parked on the restart line itself — deliberately *not* stepped
    /// past it, unlike `mode3_length::engine`.
    fn just_enabled() -> Gpu {
        let mut gpu = Gpu::new();
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.wb(0xFF40, 0x91); // LCD on, BG on, tile data $8000
        gpu
    }

    /// The restart line has no OAM scan — `STAT` reads mode 0 through the OAM
    /// slot — but it **does** draw: mode 3 starts at dot 80 of that very line,
    /// and that transition is the only thing that clears the "no OAM scan"
    /// latch.
    ///
    /// Before the fix the engine's mode-0 fast-forward swallowed dot 80, so the
    /// latch survived, the next line was skipped the same way, and so was every
    /// line after it: `LY` ticked over with the PPU parked in mode 0 until the
    /// VBlank at line 144 finally moved it. A whole frame with no mode 2 and no
    /// mode 3.
    #[test]
    fn the_ppu_restarts_into_mode_3_on_the_line_the_lcd_came_on() {
        let mut gpu = just_enabled();
        let mut at = None;
        for _ in 0..super::LINE_DOTS {
            gpu.do_cycle(1);
            if gpu.mode() == 3 {
                at = Some(gpu.modeclock);
                break;
            }
            assert_eq!(gpu.mode(), 0, "STAT must read mode 0 until mode 3 begins");
        }
        assert!(
            at.is_some(),
            "the PPU never left mode 0 on the line the LCD came on"
        );
        // Dot 80 is the first dot of mode 3; `modeclock` has moved past it by
        // the time the caller can observe the new mode.
        assert_eq!(
            at,
            Some(super::OAM_DOTS + 1),
            "and the line is not restarted"
        );
        assert_eq!(gpu.rb(0xFF44), 0, "still on the line the LCD came on");
    }

    /// And the consequence a program can see: with only the mode-2 STAT source
    /// enabled, the strobe must arrive on the **next** line, not a frame later.
    ///
    /// Every Mealybug ROM arms itself exactly this way and then falls through a
    /// field of `nop`s waiting for it; miss the strobe and the program counter
    /// runs off the end of the slide and the ROM crashes before it has drawn
    /// anything. That is what "3/79" was measuring for most of the suite.
    #[test]
    fn the_mode_2_strobe_arrives_on_the_line_after_the_lcd_comes_on() {
        let mut gpu = just_enabled();
        gpu.wb(0xFF41, 0x20); // STAT: mode-2 source only
        gpu.interrupt = 0;
        let mut fired = None;
        for dot in 0..2 * super::LINE_DOTS {
            gpu.do_cycle(1);
            if gpu.interrupt & 0x02 != 0 {
                fired = Some(dot);
                break;
            }
        }
        assert!(
            fired.is_some(),
            "no mode-2 STAT interrupt in the two scanlines after the LCD came on"
        );
    }
}
