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
    /// `SCY + LY` as latched at the tile-number fetch.
    fetch_y: u8,
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
    /// Read by [`Gpu::hblank_strobe`](super::Gpu::hblank_strobe), which needs
    /// the dot the fetcher finishes rather than the dot `mode` changes on.
    pub(super) lx: u8,
    /// How many pixels have been shifted out and **dropped** at the head of the
    /// line so far, counted modulo 8. The head-of-line phase ends when this
    /// reaches `SCX & 7`, which is re-read on every one of those dots — see
    /// [`Gpu::output_dot`].
    discard: u8,
    /// **Whether window content is still being displayed** — set true the
    /// instant the window takes over and cleared only once the background has
    /// genuinely resumed: at the next natural tile boundary (a completed fetch
    /// about to push, with the FIFO already drained) found with `win_active`
    /// already false. That gating — not a per-dot check — is what keeps this
    /// flag's stop always landing on a whole tile, so the pixel it stops on is
    /// always on the window's own `WX - 7 + 8n` grid with no arithmetic needed;
    /// see `the_window_tile_grid_is_anchored_at_wx_minus_7`. `check_window_start`
    /// reads this (not `win_active`) to refuse a re-trigger, so a re-enable
    /// mid-drain waits for the same boundary rather than interrupting it.
    window: bool,
    /// **Whether the FETCHER should address window tiles right now** — flips
    /// the instant `LCDC.5` is read low, mid-fetch or not, unlike `window`
    /// above. `fetch_tile_number` reads this (immediate) rather than `window`
    /// (deferred): a mid-fetch abort must redirect the very next tile-number
    /// read to the background, not wait for the window's already-queued
    /// content to drain first. docboy's per-stage `!lcdc.win_enable` checks
    /// are this flag's model (`src/docboy/docboy/ppu/ppu.cpp`).
    win_active: bool,
    /// **Which side the CURRENTLY IN-FLIGHT (or just-completed) fetch was
    /// addressed as**, captured once at `fetch_tile_number` and left alone for
    /// the rest of that tile's fetch — `win_active` can flip again before the
    /// tile pushes (an abort caught after the tile number was already read),
    /// and `lo_hi`'s row and the push-time `fetch_x` guard must still agree
    /// with what the tile number itself was read against, or the row and the
    /// map read come from different sides of one tile.
    tile_is_window: bool,
    /// The window was drawn somewhere on this line — which is what advances the
    /// window's internal line counter.
    win_drawn: bool,
    /// Pixels of the window's **first** tile that are left of the screen.
    /// The window's tile grid is anchored at `WX - 7`; with `WX < 7` that is
    /// off the left edge, so the first tile is partly invisible. They are
    /// shifted out and dropped a dot each, exactly like the `SCX & 7` discards
    /// that follow them. See [`Gpu::check_window_start`].
    win_skip: u8,
    /// Set by the window's takeover and cleared by its first push. Exempts
    /// that one tile from the window pixel-insertion glitch (below) **and**
    /// from advancing `fetch_x` (see `Gpu::fetch_tile_number` /
    /// `Gpu::bg_fetch_dot`'s push) — docboy's `w.just_activated`, whose own
    /// comment on the shared tile-counter advance says a fetch the window
    /// aborts mid-cycle still counts, but the window's first tile after
    /// activation never does.
    win_fresh: bool,
    /// The window's OWN tile counter, separate from `fetch_x` — reset to 0 on
    /// every activation and read (then advanced) at the window's tile-number
    /// fetch. Splitting it out is what lets the background's counter carry on
    /// underneath a window that is currently drawing, instead of losing its
    /// place every time the window takes over: `fetch_x` now advances on
    /// every completed tile fetch regardless of which one addressed it,
    /// exactly as it would have if the background had kept fetching all
    /// along, so a window that stops mid-line resumes the background exactly
    /// where it would already be. Modelled on docboy's `wf.tilemap_x` beside
    /// `bwf.lx` (`src/docboy/docboy/ppu/ppu.cpp`), read and reimplemented per
    /// `CONTRIBUTING.md`.
    win_fetch_x: u8,
    /// Whether any tile has been pushed yet on this scanline. `prologue_pos`
    /// used to infer "still in the head-of-line prologue" from `bg_len == 0`,
    /// which is also briefly true the instant a push drains the FIFO to empty
    /// — a state the window's own take-over check must never mistake for the
    /// prologue again. This flag makes the distinction explicit instead of
    /// inferred, which is what keeps a mid-line window stop (or a sprite
    /// fetch delaying the next push) from being read as the very start of
    /// the line.
    line_started: bool,
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
            fetch_y: 0,
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
            win_active: false,
            tile_is_window: false,
            win_drawn: false,
            win_skip: 0,
            win_fresh: false,
            win_fetch_x: 0,
            line_started: false,
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
        w.u8(self.fetch_y);
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
        w.bool(self.win_active);
        w.bool(self.tile_is_window);
        w.bool(self.win_drawn);
        w.u8(self.win_skip);
        w.bool(self.win_fresh);
        w.u8(self.win_fetch_x);
        w.bool(self.line_started);
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

    /// `version` is the state's own format version — see
    /// [`crate::savestate::MIN_SUPPORTED_VERSION`]. v24 split one field
    /// (`window`) into the four read here conditionally; on an older state
    /// each defaults to what the pre-split code actually did with `window`
    /// alone, so a v22/v23 load reconstructs the same behaviour that version
    /// of this engine had, not a new approximation of it.
    pub(super) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
        version: u16,
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
        self.fetch_y = r.u8()?;
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
        // v24 split the fetcher's immediate addressing (`win_active`) and its
        // per-tile freeze (`tile_is_window`) out of `window`, which before
        // that split was what `fetch_tile_number`/`lo_hi` read directly for
        // the same decision — so on an older state, mirroring the `window`
        // value just read above is not a guess, it is exactly what those
        // functions would have used. The only way this can differ from a true
        // v24+ machine at the same instant is the one case the split exists
        // for: a mid-fetch abort caught after the tile number was already
        // read but before `window` itself has settled — a single in-flight
        // tile fetched against the wrong side, never more.
        self.win_active = if version >= 24 {
            r.bool()?
        } else {
            self.window
        };
        self.tile_is_window = if version >= 24 {
            r.bool()?
        } else {
            self.window
        };
        self.win_drawn = r.bool()?;
        self.win_skip = r.u8()?;
        self.win_fresh = r.bool()?;
        // v24's other two fields, same commit, same reasoning: before the
        // split there was one tile counter (`fetch_x`) shared by both
        // fetchers, so defaulting `win_fetch_x` to the just-loaded `fetch_x`
        // reproduces that shared-counter behaviour exactly rather than
        // inventing a fresh window counter mid-line. `line_started` replaced
        // an *inference* from `bg_len == 0` — recomputing that same inference
        // here (`bg_len` was already loaded above) reproduces the identical
        // ambiguity the pre-v24 code always had, including its one known
        // false positive right after a push drains the FIFO to empty; that is
        // the honest default, not a corrected one.
        self.win_fetch_x = if version >= 24 { r.u8()? } else { self.fetch_x };
        self.line_started = if version >= 24 {
            r.bool()?
        } else {
            self.bg_len > 0
        };
        Ok(())
    }

    /// Test-only mirror of [`Pipeline::save_state`] that writes exactly what
    /// a real writer at `version` produced — the four v24 fields omitted
    /// below `version >= 24`, everything else unconditional, matching
    /// [`Pipeline::load_state`]'s gates field for field. This exists so the
    /// savestate-compatibility tests can construct genuinely SHORTER
    /// old-format byte streams (not merely bytes stamped with an old version
    /// number) without vendoring a binary fixture. **Never compiled outside
    /// `cfg(test)`** — production `save_state` stays unconditional; only the
    /// reader carries backward compatibility, per
    /// [`crate::savestate::MIN_SUPPORTED_VERSION`]'s doc comment.
    #[cfg(test)]
    pub(super) fn save_state_as_of(
        &self,
        w: &mut crate::savestate::Writer,
        version: u16,
    ) {
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
        w.u8(self.fetch_y);
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
        if version >= 24 {
            w.bool(self.win_active);
            w.bool(self.tile_is_window);
        }
        w.bool(self.win_drawn);
        w.u8(self.win_skip);
        w.bool(self.win_fresh);
        if version >= 24 {
            w.u8(self.win_fetch_x);
            w.bool(self.line_started);
        }
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
                self.tick_dma_arm(step);
                if self.modeclock >= LINE_DOTS {
                    self.next_line();
                }
                self.refresh_stat_line();
                self.tile_sel_glitch = false;
                continue;
            }
            // Mode 2 has nothing a dot can distinguish — take the whole run.
            if self.modeclock < OAM_DOTS {
                let step = left.min(OAM_DOTS - self.modeclock);
                self.oam_scan_span(step);
                left -= step;
                continue;
            }
            // **Mode 3 is a RUN, not a sequence of independent dots**, and
            // everything the general path re-decides each dot is invariant
            // across it: `pipeline_dot` reaches no `change_mode` and no
            // `next_line` (the only calls are in this function, `accurate_dot`
            // and `oam_scan_span`), so within mode 3 the mode cannot move
            // under the loop and `line` cannot either. The two ways out are
            // `lx` reaching the screen edge and `modeclock` reaching the end
            // of the line, and both are the loop's own condition.
            //
            // So the general path's `idle` test, its OAM-slot test, and
            // `accurate_dot`'s own repeat of both plus its `mode != 3` arm are
            // all answering a question that was settled when the run began.
            // Mode 3 is ~172-289 dots of every visible line, which is the bulk
            // of the frame's dots, so that is ~six predictable-but-real
            // branches paid about twenty-five thousand times a frame.
            //
            // This is a control-flow change and nothing else: the work per dot
            // is `pipeline_dot` then `end_of_dot`, in that order, exactly as
            // `accurate_dot` runs them.
            // The `lx` half of this guard is load-bearing and its absence is
            // an infinite loop, not a slow path: mode 3 with `lx` already at
            // the screen edge is exactly the state whose *only* transition is
            // `accurate_dot`'s `change_mode(0)`, so a fast path that took it
            // would spin the outer loop forever without ever consuming `left`.
            if self.mode == 3 && (self.pipe.lx as usize) < WIDTH {
                while left > 0 && self.mode == 3 && (self.pipe.lx as usize) < WIDTH {
                    self.pipeline_dot();
                    self.end_of_dot();
                    self.tick_dma_arm(1);
                    left -= 1;
                }
                continue;
            }
            self.accurate_dot();
            self.tick_dma_arm(1);
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
            self.oam_scan_span(1);
            return;
        }
        if self.mode == 3 && self.pipe.lx as usize >= WIDTH {
            // Mode 3 is over. **The transition is applied at the TOP of the
            // first mode-0 dot, not at the bottom of the last mode-3 one** —
            // the same edge convention mode 2 → 3 already uses above, and the
            // asymmetry was a real one-dot error. Because the caller folds `IF`
            // and reads registers once per M-cycle, a dot is a whole M-cycle to
            // the CPU: `acceptance/ppu/lcdon_timing-GS` reads `STAT` at dot 252
            // of a line and requires mode **3**, which is only true if dot 251
            // is still a mode-3 dot when observed.
            //
            // The mode-0 *interrupt* does not move with it — it is one dot
            // earlier, on the dot the fetcher finished. See
            // [`Gpu::hblank_strobe`](super::Gpu::hblank_strobe).
            self.change_mode(0);
        } else {
            if self.mode != 3 {
                self.skip_first_oam = false;
                self.change_mode(3);
            }
            self.pipeline_dot();
        }

        self.end_of_dot();
    }

    /// Everything a dot does *after* its own work, whichever mode it was in.
    ///
    /// Extracted so the mode-3 run in [`Gpu::do_cycle_accurate`] and the
    /// general [`Gpu::accurate_dot`] cannot drift: there is one definition of
    /// what ending a dot means, and both call it.
    #[inline(always)]
    fn end_of_dot(&mut self) {
        self.modeclock += 1;
        if self.modeclock >= LINE_DOTS {
            self.next_line();
        }
        self.refresh_stat_line();
        // One dot only — see [`Gpu::tile_sel_glitch`].
        self.tile_sel_glitch = false;
        // Snapshot `WX` for the NEXT dot's [`Gpu::check_window_start`] to
        // compare against — one dot after whatever this dot's own commit
        // landed. Unconditional, every accurate dot, matching docboy's own
        // `last_wx = wx;` placement at the end of its per-dot `tick()`. See
        // `winx_prev`'s doc comment.
        self.winx_prev = self.winx;
    }

    /// The whole run of **mode-2 dots** the caller's budget reaches, at once.
    ///
    /// Mode 2 is 80 dots of one job — walk the 40 OAM entries, two dots each —
    /// and, unlike mode 3, nothing it does is visible dot by dot: the scan list
    /// is only read when `start_mode3` sorts it, the STAT line's mode-2 term is
    /// asserted for the whole window (see [`Gpu::oam_strobe`](super::Gpu::oam_strobe)),
    /// and the `LY`/`LYC` term cannot move inside a line. So the only thing a
    /// per-dot walk of mode 2 bought was 79 evaluations of a signal that could
    /// not have changed — **56 % of the whole per-dot engine's cost, measured**.
    ///
    /// It is exact rather than approximate because the caller cannot change
    /// anything mid-span: `do_cycle_accurate` is reached from
    /// [`MemoryManagementUnit::bus_tick`](crate::mmu::MemoryManagementUnit) with
    /// one M-cycle's worth of dots, or from the idle batch, which is bounded by
    /// [`Gpu::idle_dots`](super::Gpu::idle_dots) — and that bound stops at the
    /// end of mode 2. So `LCDC.2` (which decides sprite height and is read live,
    /// per entry, because `m3_lcdc_obj_size_change` moves it), `WY`, `LYC` and
    /// OAM itself all hold still for the length of one call.
    ///
    /// `dots` must not carry the span past `OAM_DOTS`; `do_cycle_accurate`
    /// clamps it.
    fn oam_scan_span(&mut self, dots: u32) {
        let from = self.modeclock;
        let to = from + dots;
        debug_assert!(to <= OAM_DOTS);

        // The line the PPU restarts on after `LCDC` bit 7 goes 0→1 has no OAM
        // scan at all: `STAT` reads mode 0 and the mode-2 source does not
        // strobe (`acceptance/ppu/lcdon_timing-GS`). `check_wy_trigger` latches
        // once and is idempotent, so once for the span is once per dot.
        if self.skip_first_oam {
            self.check_wy_trigger();
        } else {
            if self.mode != 2 {
                self.start_oam_scan();
                self.change_mode(2);
            }
            // Dots 0..79 -> entries 0..39, one per two dots: the even ones.
            let mut dot = from + (from & 1);
            while dot < to {
                // The span may straddle the dot an armed OAM DMA takes the
                // address bus on, and the scan is the one thing in mode 2 that
                // can tell: `oamdma/late_sp*` is 46 rows of exactly that edge.
                self.tick_dma_arm(dot - self.modeclock);
                self.modeclock = dot;
                self.oam_scan_entry((dot / 2) as usize);
                dot += 2;
            }
        }

        self.tick_dma_arm(to - self.modeclock);
        self.modeclock = to;
        if to == OAM_DOTS {
            // Mode 3 begins on the next dot; set the pipeline up now so its
            // first fetch happens on that dot rather than a dot late.
            self.start_mode3();
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

    /// One OAM entry. Hardware checks one sprite every two dots, so forty of
    /// them fill the 80-dot mode; `LCDC.2` is read live, which is what
    /// `m3_lcdc_obj_size_change` changes underneath it.
    fn oam_scan_entry(&mut self, index: usize) {
        // **The scan reads the OAM bus, and while an OAM DMA runs it does not
        // get to.** The DMA owns the address lines, so the Y/X latch keeps
        // whatever it held — which is the last entry of the *previous* line's
        // scan, or the last tile number a mode-3 object fetch put there. Every
        // slot the scan then walks is judged against that one stale pair.
        //
        // That is the whole of `strikethrough.gb`: it starts a transfer in the
        // HBlank before a line whose forty objects all sit on it, so the latch
        // is left holding object 39's `Y`/`X` and the next line finds ten
        // phantom objects stacked at that single `X`.
        //
        // Modelled after SameBoy's `mode2_y_bus`/`mode2_x_bus`
        // (`Core/display.c`, `add_object_from_index`); reimplemented here.
        if !self.dma_owns_oam {
            self.oam_bus_y = self.oam_read(index * 4);
            self.oam_bus_x = self.oam_read(index * 4 + 1);
        }
        if index >= 40 || self.pipe.nscan >= 10 {
            return;
        }
        let y = self.oam_bus_y as i32 - 16;
        let line = self.line as i32;
        if line < y || line >= y + self.sprite_size as i32 {
            return;
        }
        let n = self.pipe.nscan as usize;
        self.pipe.scan[n] = ScanSprite {
            y: self.oam_bus_y,
            x: self.oam_bus_x,
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
        p.win_active = false;
        p.tile_is_window = false;
        p.win_drawn = false;
        p.win_skip = 0;
        p.win_fresh = false;
        p.win_fetch_x = 0;
        p.line_started = false;
        p.fetched = 0;
        // Nothing about `SCX` is latched here. Its low three bits decide how
        // many pixels the head of the line throws away, and that comparison is
        // made **live, on each of those dots** — see [`Gpu::output_dot`].
        p.discard = 0;
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
        // `winx_prev` is not tracked through HBlank/VBlank (this engine skips
        // those in long batches, never calling the per-dot tick that would
        // otherwise keep it current) — settle it to the live register here so
        // a write made outside mode 3 is never artificially stale by the time
        // `check_window_start` can first run this line. See its doc comment.
        self.winx_prev = self.winx;
    }

    /// One dot of the pixel pipeline.
    ///
    /// Order matters and is not arbitrary: a sprite fetch freezes both the
    /// output and the background fetcher, the output shifts a pixel that was
    /// already in the FIFO *before* this dot's fetch could push, and the
    /// fetcher then gets its dot. That ordering is what makes the first pixel
    /// appear on dot 12 and mode 3 come out at 172 dots for `SCX & 7 == 0`.
    fn pipeline_dot(&mut self) {
        #[cfg(test)]
        if std::env::var_os("SCRATCH_DOT_TRACE").is_some()
            && self.line
                == std::env::var("SCRATCH_LINE")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(8)
        {
            eprintln!(
                "PREdot={:4} win={} win_on={} fetch_x={:3} fetch_t={} \
                 win_skip={} lx={:3} discard={} bg_len={} winx={}",
                self.modeclock,
                self.pipe.window,
                self.win_on,
                self.pipe.fetch_x,
                self.pipe.fetch_t,
                self.pipe.win_skip,
                self.pipe.lx,
                self.pipe.discard,
                self.pipe.bg_len,
                self.winx,
            );
        }
        if self.check_window_start() {
            // A `WX = 0` takeover on a line with fractional scrolling spends
            // this dot and nothing else. See [`Gpu::check_window_start`].
            return;
        }

        // A sprite fetch in progress owns the dot outright.
        if self.pipe.obj_t > 0 {
            self.obj_fetch_dot();
            return;
        }

        // A sprite is due here: the output stops until it has been fetched.
        if let Some(i) = self.due_sprite() {
            if self.pipe.bg_len > 0 {
                self.start_obj_fetch(i);
                // The background fetcher still gets this dot. docboy
                // (`src/docboy/docboy/ppu/ppu.cpp`,
                // `handle_pixel_slice_fetcher_push`) starts the object fetch
                // from inside the fetcher's own tick — "Note: the first obj
                // prefetcher tick overlaps this tick" — so the dot on which an
                // object is detected is a fetcher dot as well. Freezing the
                // fetcher here instead put every tile-number read after an
                // object one dot late, which is exactly what
                // `m3_scx_high_5_bits` measures: with a mid-line `SCX` write
                // landing in that dot we read the *new* high five bits for a
                // tile hardware had already fetched with the old ones.
                self.bg_fetch_dot();
                return;
            }
        }

        self.output_dot();
        self.bg_fetch_dot();
    }

    /// How far the head-of-line prologue has got, as a position **left of
    /// pixel 0**: `Some(0)` on the last dot of the discarded first fetch,
    /// counting up to `Some(7)` on the dot the first pixel becomes available,
    /// and `None` from then on — where `lx` takes over.
    ///
    /// Eight positions, because that is how many the second fetch covers: it
    /// runs for six dots and the FIFO fills at the end of it. A window at
    /// `WX = n` for `n <= 7` starts at pixel `n - 7`, i.e. at position `n`
    /// here.
    fn prologue_pos(&self) -> Option<i16> {
        // `line_started` (not `bg_len == 0`) is what says whether this is
        // genuinely the head-of-line prologue. The two used to coincide, but
        // only because the window's own stop used to force `bg_len` back to
        // zero for several dots to force a resync (`bg_clear`, now gone) —
        // and while that was going on, `bg_len == 0` read as "still in the
        // prologue" and let the window's take-over equality fire again
        // mid-line, whenever `WX` happened to match the fetcher's phase
        // during the gap. `line_started` is the fact itself rather than an
        // inference that only happened to hold for as long as nothing else
        // ever put the FIFO through a multi-dot empty spell.
        if self.pipe.line_started {
            return None;
        }
        if self.pipe.first_fetch {
            // The discarded fetch runs six dots and only its last one is
            // position 0; the rest are further left still and must stay
            // negative, or a window at `WX = 7` matches `lx == 0` on the very
            // first dot of the line and truncates the prologue.
            Some(self.pipe.fetch_t as i16 - (FETCH_DOTS as i16 - 1))
        } else {
            Some(self.pipe.fetch_t as i16 + 1)
        }
    }

    /// The window takes over the fetcher the moment the output reaches `WX-7`,
    /// provided the frame-long `WY` condition has already latched.
    ///
    /// Re-enabling `LCDC.5` later on the same line starts the window's **next**
    /// row, which is why the line counter is advanced here — at every
    /// activation — rather than once per scanline. Mealybug's
    /// `m3_lcdc_win_en_change_multiple` is built entirely out of that.
    ///
    /// Returns `true` when the takeover consumed the whole dot: with `WX = 0`
    /// on a line whose `SCX & 7` is non-zero the window activates **one T-cycle
    /// later**, which is `m3_window_timing_wx_0`'s own stated subject ("due to
    /// window activating one T-cycle later when WX = 0 and SCX > 0") and which
    /// SameBoy charges in the same place — `Core/display.c`, immediately after
    /// `fifo_clear(&gb->bg_fifo)`: `if (WX == 0 && (SCX & 7) …) { cycles_for_line
    /// += 1; GB_SLEEP(…, 1); }`. Skipping the fetcher's dot rather than adding a
    /// counter is the whole of it, so it costs no state.
    fn check_window_start(&mut self) -> bool {
        if self.pipe.window || !self.win_on || !self.wy_trigger {
            return false;
        }
        // **Read the one-dot-stale snapshot, not the live register, for
        // everything below.** `winx_prev` is `WX` as of the end of the
        // previous dot — see its doc comment for the full citation. This is
        // what `m3_wx_5_change`/`m3_wx_6_change` are built to distinguish: a
        // mid-line `WX = LY` write is timed to land on the **exact** dot an
        // earlier `WX = 5` (or `6`) pulse's own trigger position is due, so
        // whichever value this check sees decides the whole picture. Using
        // the live, already-committed `winx` here made every line's early
        // pulse lose the race against the write that immediately overwrote
        // it, so the window activated on the wrong side of the screen for
        // the whole rest of the line — `m3_wx_5_change@DMG` was
        // 13,660/23,040 px, since almost every line hits this exact race
        // (the two ROMs' handlers use identical cycle counts throughout).
        let winx = self.winx_prev;
        // The comparison is made **at the output**, against the pixel about to
        // be shifted out — so it cannot happen before there is one. Checking it
        // during the head-of-line fetch, where `lx` is already 0 and the FIFO is
        // still empty, let a window with `WX <= 7` take over a fetcher that had
        // nothing to throw away and so cost nothing: `m3_window_timing` sets
        // `WX = LY` on every line and photographs the difference between a
        // window that started before the palette write and one that started
        // after, which is exactly the six dots of the window's own startup
        // fetch. Without this the boundary sat at the same pixel for every `WX`.
        if winx > 166 {
            return false;
        }
        // Where the output has got to, counting the eight positions left of
        // pixel 0 that the head-of-line prologue occupies. `prologue_pos`
        // returns 0..=7 through those and `None` once the first pixel is
        // available, after which `lx` is the position. A window whose left edge
        // is at or left of pixel 0 is therefore reached *during* the prologue,
        // which is the only way its tile grid can be anchored at `WX - 7` and
        // its first pixel still land on the dot it does.
        let reached = match self.prologue_pos() {
            Some(p) => p == winx as i16,
            None => self.pipe.lx as i16 == winx as i16 - 7,
        };
        if !reached {
            return false;
        }
        // **The comparison is an equality, not a threshold.** The window
        // starts on the pixel `WX - 7` and on no other, which is what makes
        // re-enabling `LCDC.5` later in the same scanline a no-op unless `WX`
        // has been moved somewhere the output has not reached yet — mealybug's
        // own notes: "Setting WIN_EN again during mode 3 on the same scanline
        // will have no effect unless WX has been updated to set the window to
        // activate on a pixel that hasn't been drawn yet"
        // (`the-comprehensive-game-boy-ppu-documentation.md`, WIN_EN). With a
        // threshold, `lx` has already passed the old `WX - 7` by then and the
        // window restarts on the very next dot however `WX` was left. SameBoy
        // spells it the same way (`Core/display.c`:
        // `WX == (uint8_t)(gb->position_in_line + 7)`).
        //
        // `WX <= 7` puts the start at or left of the first pixel, where the
        // equality still has to fire: those are the positions
        // `m3_window_timing_wx_0` walks.
        self.wy_pos += 1;
        let stall = winx == 0 && self.scx & 7 != 0;
        let skip = 7u8.saturating_sub(winx);
        let p = &mut self.pipe;
        p.window = true;
        p.win_active = true;
        p.win_drawn = true;
        // The window's OWN counter resets on every activation; `fetch_x` —
        // the background's — is deliberately left untouched, so a background
        // that the window interrupts resumes exactly where it would already
        // be. See `Pipeline::win_fetch_x`.
        p.win_fetch_x = 0;
        p.fetch_t = 0;
        // A takeover inside the prologue truncates the discarded first fetch —
        // the window's own fetch is what runs from here.
        p.first_fetch = false;
        p.win_skip = skip;
        p.win_fresh = true;
        p.bg_clear();
        stall
    }

    /// Advance the background/window fetcher one dot, reading each register at
    /// the stage hardware reads it at.
    ///
    /// **Turning `LCDC.5` off is checked here, every dot, not only once a tile
    /// finishes.** docboy checks `!lcdc.win_enable` at the top of every one of
    /// its window fetcher stages (`win_prefetcher_get_tile_0`/`_1`,
    /// `win_pixel_slice_fetcher_get_tile_data_{low,high}_{0,1}` —
    /// `src/docboy/docboy/ppu/ppu.cpp`) and falls through to the matching
    /// background stage the instant it is false, so a tile whose fetch is
    /// already in flight when the write commits finishes as background from
    /// whichever stage comes next — it does **not** wait for the tile that is
    /// already fully fetched-as-window to be thrown away and refetched. A tile
    /// whose fetch had *already completed* as window before the write commits
    /// is unaffected: nothing left to abort. Doing this as a flag flip rather
    /// than redirecting per stage is equivalent here, because `fetch_tile_number`
    /// and `lo_hi` already branch on `self.pipe.win_active` (and its frozen
    /// per-tile copy, `self.pipe.tile_is_window`) themselves — flipping
    /// `win_active` before whichever of those runs next is all "fall through
    /// to the background stage" amounts to. `self.pipe.window` is a separate,
    /// deferred question — see its own doc comment — and is resolved later in
    /// this same function, only once the FIFO reaches a natural boundary.
    fn bg_fetch_dot(&mut self) {
        // This is the FETCHER's own abort, checked every dot regardless of
        // how much already-fetched window content is still queued in the
        // FIFO — `win_active` is the immediate flag `fetch_tile_number`
        // addresses off, and it must flip the instant `LCDC.5` reads low,
        // mid-fetch or not. `window` (the externally-visible flag) is a
        // different, deferred question and is resolved separately, below,
        // once the FIFO reaches a natural tile boundary.
        if self.pipe.win_active && !self.win_on {
            self.pipe.win_active = false;
            // Once the high bitplane has been read there is nothing left for
            // an abort to change — the tile is fully read, whichever side it
            // was read as, and it pushes as that. Before that, restart the
            // cycle: the tile number is read once, at the *start* of a fetch,
            // and every later stage reuses it regardless of which register it
            // otherwise branches on, so an abort caught earlier would
            // otherwise finish the tile with the window's tile number and the
            // background's row — a tile neither side ever draws. Restarting
            // costs nothing extra here (the next dot was already going to
            // read a register), and is what keeps the background clean
            // whenever it has to take over mid-fetch rather than only at a
            // tile boundary.
            let late = u8::from(!self.gbmode.is_dmg());
            if self.pipe.fetch_t > 0 && self.pipe.fetch_t < 4 + late {
                self.pipe.fetch_t = 0;
            }
        }
        if self.pipe.fetch_t < FETCH_DOTS {
            self.pipe.fetch_t += 1;
            // Each 2-dot stage does its VRAM read on the stage's FIRST dot on
            // DMG silicon and its SECOND on colour silicon — the same one-dot
            // difference docboy documents from the register side ("CGB is not
            // affected — it sees LCDC delayed by 2 T-Cycles by default",
            // `src/docboy/docboy/ppu/ppu.cpp`; their fetcher reads at phase 0
            // of each stage). Measured here: moving the DMG sample to the
            // first dot takes `m3_scy_change` 2,130 → 248 px, `m3_scx_high_5_bits`
            // 86 → 12 and `m3_lcdc_tile_sel_change` 22,316 → 802, while the
            // same move on the colour machine loses ground on every affected
            // CGB row.
            let late = u8::from(!self.gbmode.is_dmg());
            match self.pipe.fetch_t - late {
                1 => self.fetch_tile_number(),
                3 => self.lo_hi(false),
                5 => self.lo_hi(true),
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
        // Every tile that reaches a push, on any scanline, has to have run
        // through here at least once. See `Pipeline::line_started`.
        self.pipe.line_started = true;
        // **The window pixel-insertion glitch.** Once `WY == LY` has latched for
        // the frame, a push landing on the pixel the window starts at supplies a
        // single colour-0 pixel instead of the tile, and the real push is
        // postponed a dot — so everything from there on is one pixel to the
        // right. It fires whether or not the window is currently drawing, and
        // the window's *own* first tile is exempt. docboy,
        // `src/docboy/docboy/ppu/ppu.cpp`, `handle_pixel_slice_fetcher_push`:
        // "if this push stage happens when LX == WX, then a 00 pixel is pushed
        // into the bg fifo instead of the fetched tile … The first window tile
        // after the window activation seems to be an exception."
        // (SameBoy reaches the same place through `gb->insert_bg_pixel`.)
        // `mealybug/m3_wx_4_change@DMG` is 229 pixels of exactly this: one
        // inserted pixel at column `WX - 7` on every line whose mid-mode-3 `WX`
        // write lands there.
        // **On Color silicon the glitch needs the window to be DRAWING.**
        // docboy, same function: `if (w.active_for_frame && lx == wx &&
        // !w.just_activated #ifdef ENABLE_CGB && w.active #endif)`, with the
        // comment "Verified: this happens also on CGB, although the conditions
        // that make this happen are slightly different. In particular, on CGB
        // the glitch seems to happen only if we are fetching a window tile
        // right now." Ungated, a Color console inserts a pixel at `WX - 7` on
        // every line where the Y condition has latched and the window is
        // *disabled* — which is what `cgb-acid2` measures: with `SCX & 7 == 0`
        // a push lands exactly on column 88 (`WX = 95`) and the whole of the
        // rest of every line comes out one pixel to the right, 158 pixels of
        // it. `is_dmg()` is the console, not the cartridge, so a Color machine
        // running a DMG cartridge (`ColorAsClassic`) takes the gated path —
        // the same predicate `bg_fetch_dot` uses for the fetcher's read phase.
        if self.wy_trigger
            && !self.pipe.win_fresh
            && (self.gbmode.is_dmg() || self.pipe.tile_is_window)
            && self.pipe.lx as i16 == self.winx as i16 - 7
        {
            self.pipe.bg[self.pipe.bg_head as usize & 7] = Pixel::default();
            self.pipe.bg_len = 1;
            return;
        }
        // `fetch_x` is the BACKGROUND's own counter now, addressed only from
        // the background branch of `fetch_tile_number`; a window tile reads
        // and advances `win_fetch_x` instead. So while the tile about to push
        // was fetched AS a window tile, `fetch_x` must not move at all — not
        // just on the window's first tile since activation, on every one of
        // them, or the background's resume position is polluted by the time
        // the window stops and `fetch_x` is consulted again. `tile_is_window`
        // (not the live `win_active`) is what a late abort needs here: it was
        // frozen at this tile's own tile-number read and survives an abort
        // that catches it too late to restart. `win_fresh` still gates the
        // pixel-insertion glitch above; it plays no part in this decision.
        self.pipe.win_fresh = false;
        self.push_bg_tile();
        if !self.pipe.tile_is_window {
            self.pipe.fetch_x = self.pipe.fetch_x.wrapping_add(1);
        }
        self.pipe.fetch_t = 0;
        // **Where `window` (the deferred, externally-visible flag) actually
        // resolves to false.** This point is unreachable while `bg_len > 0`
        // (the early return above), so it only ever runs at a natural tile
        // boundary — a fetch has just completed and pushed with the FIFO
        // already empty beforehand. That gating, not a per-dot check, is what
        // makes the background always resume on a whole tile: any
        // already-queued window content keeps draining normally (through
        // ordinary pops in `Gpu::output_dot`) for as long as it takes, and
        // `window` only comes down once there is truly nothing "window" left
        // to account for. Modelled on the old `check_window_stop`'s own
        // gating (called from this exact spot before this refactor), kept
        // because it is what already made
        // `the_window_tile_grid_is_anchored_at_wx_minus_7` pass — the
        // grid-anchoring fix changes where the window *starts* and how a
        // mid-fetch abort avoids mixing a tile's two halves, not this.
        if self.pipe.window && !self.pipe.win_active {
            self.pipe.window = false;
        }
    }

    fn fetch_tile_number(&mut self) {
        // Frozen here, once per tile, for `lo_hi` and the push-time `fetch_x`
        // guard to read back — seeing `win_active`'s value now rather than
        // whatever it happens to be when they run, dots later.
        self.pipe.tile_is_window = self.pipe.win_active;
        let (map, tx, ty) = if self.pipe.win_active {
            let wy = self.wy_pos.max(0) as u16;
            let tx = self.pipe.win_fetch_x as u16 & 31;
            self.pipe.win_fetch_x = self.pipe.win_fetch_x.wrapping_add(1);
            (self.win_tilemap, tx, (wy >> 3) & 31)
        } else {
            let bgy = self.scy.wrapping_add(self.line) as u16;
            (
                self.bg_tilemap,
                ((self.scx as u16 >> 3) + self.pipe.fetch_x as u16) & 31,
                (bgy >> 3) & 31,
            )
        };
        // Latched for the bitplane fetches on CPU CGB D and later — see
        // `lo_hi` and [`PpuRevision`].
        self.pipe.fetch_y = self.scy.wrapping_add(self.line);
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
        // `SCY + LY` is re-read here, not latched at the tile-number fetch —
        // which is what lets a one-M-cycle `SCY` change mix two different rows
        // into a single drawn tile (`m3_scy_change`). **Colour silicon from
        // CPU CGB D onward latches it instead**, so the desync is not
        // available there at all; see [`PpuRevision`], which cites SameBoy and
        // docboy for the boundary.
        let cached = !self.gbmode.is_dmg() && self.cgb_ppu_revision().caches_fetcher_y();
        let row = if self.pipe.tile_is_window {
            (self.wy_pos.max(0) as u16) & 7
        } else if cached {
            self.pipe.fetch_y as u16 & 7
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
        // **`LCDC.4` reset on this very dot.** Matt Currie's `TILE_SEL` section
        // (see [`PpuRevision::tile_sel_glitch_reads_tile_index`]): every Color
        // stepping but CPU CGB D substitutes the **tile index** for the byte,
        // and D reads from the bitplane-0 address instead. `cgb-acid-hell` is
        // two pixels of the first rule and the whole of that row — its two
        // scanlines want `$55` and `$49`, which are the tile numbers the map
        // holds there. SameBoy's extra clause is kept: an index with bit 7 set
        // falls through to the ordinary read (`data_for_tile_sel_glitch`).
        // **CPU CGB D is the exception and is deliberately left unmodelled** —
        // its rule is "read the bitplane data from the address for bitplane 0",
        // and both readings of that were implemented and measured against the
        // suite's own CPU CGB D photographs. `addr & !0x1000` and SameBoy's
        // `(addr & !0x1000) - 1` give the identical, *worse* answer:
        // `m3_lcdc_tile_sel_change@CGB-D` 802 → 1,186 and
        // `m3_lcdc_tile_sel_win_change@CGB-D` 1,366 → 1,424. So D falls through
        // to the ordinary read until somebody has a third reading; see
        // `docs/mealybug.md` §13.
        if self.tile_sel_glitch
            && !self.gbmode.is_dmg()
            && self.cgb_ppu_revision().tile_sel_glitch_reads_tile_index()
            && self.pipe.tile_num & 0x80 == 0
        {
            if high {
                self.pipe.hi = self.pipe.tile_num;
            } else {
                self.pipe.lo = self.pipe.tile_num;
            }
            return;
        }
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
    }

    fn read_obj_attributes(&mut self) {
        let oam = self.pipe.scan[self.pipe.obj_i as usize].oam as usize * 4;
        // Through [`Gpu::oam_read`], because a transfer running underneath the
        // fetch drives the address the PPU gets — and one bus, so the tile
        // number lands in the same latch the scan's `Y` came out of.
        self.oam_bus_y = self.oam_read(oam + 2);
        self.pipe.obj_tile = self.oam_bus_y;
        self.pipe.obj_flags = self.oam_read(oam + 3);
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
            // **The attributes are read one dot into the fetch, not on its
            // first dot.** Nothing measures the difference unless an OAM DMA
            // is running underneath — the read then comes off the transfer's
            // address bus, and one dot moves it by a whole source byte.
            // `strikethrough.gb` is that one dot, and its DMG column is the
            // only row in any suite that can see it; SameBoy reads them in the
            // same place (`Core/display.c`, after its `GB_SLEEP(display, 41, 1)`).
            2 => self.read_obj_attributes(),
            3 => self.obj_plane(false),
            // **And the HIGH bitplane on the fetch's sixth dot, not its
            // fifth** — the same phase docboy reads it on
            // (`obj_pixel_slice_fetcher_get_tile_data_high_1_and_merge_with_obj_fifo`,
            // whose own comment names both `m3_lcdc_obj_size_change` ROMs).
            // Invisible unless `LCDC.2` moves between the two plane reads,
            // where one dot is a whole tile row: `m3_lcdc_obj_size_change_scx`
            // is 30 pixels at five and **pixel-exact at six**, on all three
            // machines. See `docs/mealybug.md` §12.
            OBJ_FETCH_DOTS => self.obj_plane(true),
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
        // **The row index is MASKED, not clamped, and Y-flip is an XOR against
        // the same mask.** `LCDC.2` is read here, live, so the scan may have
        // accepted this object as sixteen pixels tall and the fetch then find
        // it eight: the row can be 8-15 with a three-bit mask, and hardware has
        // no comparator to clamp it — the address lines simply drop the top
        // bit, wrapping to the top of the tile. SameBoy spells both halves the
        // same way (`Core/display.c`, `get_object_line_address`:
        // `tile_y = (gb->current_line - y) & (height_16? 0xF : 7)` and
        // `if (flags & 0x40) tile_y ^= height_16? 0xF : 7`).
        // `mealybug/m3_lcdc_obj_size_change_scx` is built out of exactly that
        // race and is the only ROM that measures it.
        let mask = if tall { 15 } else { 7 };
        let mut row = (self.line as i32 - (s.y as i32 - 16)) & mask;
        if flags & 0x40 != 0 {
            row ^= mask;
        }
        let row = row as u16;
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
    ///
    /// The head of the line throws `SCX & 7` pixels away first, one per dot.
    /// **`SCX` is re-read on every one of those dots rather than latched when
    /// mode 3 began**, so a write that lands in the gap between the two — and
    /// every Mealybug handler's does, because five M-cycles of interrupt
    /// dispatch put it around dot 8 while the first pixel is not shifted until
    /// dot 12 — still decides how many pixels go. SameBoy spells the same rule
    /// as a counter compared against the live register
    /// (`Core/display.c`, `render_pixel_if_possible`:
    /// `(gb->position_in_line & 7) == (gb->io_registers[GB_IO_SCX] & 7)`), and
    /// the count wraps at 8 exactly as that comparison does, so `SCX` moving
    /// *below* the number already dropped costs a further lap.
    fn output_dot(&mut self) {
        if self.pipe.bg_len == 0 {
            return;
        }
        let bg = self.pipe.bg_pop();
        if self.pipe.win_skip > 0 {
            // The window's first tile begins at `WX - 7`; with `WX < 7` its
            // leading pixels are left of the screen. They are shifted out a dot
            // each, which is what makes the takeover's head start early enough
            // for pixel 0 to land on the dot it does.
            // **The sprite FIFO is NOT shifted here.** These pixels are left
            // of the screen and an object's `X` is a screen coordinate, so
            // consuming its leading pixels against them loses them: the object
            // at `X = 8` in `m3_wx_4_change_sprites` came out three pixels
            // short on every line, which is `7 - WX` for that ROM's `WX = 4`.
            self.pipe.win_skip -= 1;
            return;
        }
        let obj = self.pipe.obj_shift();
        if self.pipe.lx == 0 && self.pipe.discard != self.scx & 7 {
            self.pipe.discard = (self.pipe.discard + 1) & 7;
            return;
        }
        let x = self.pipe.lx as usize;
        self.pipe.lx += 1;
        if x >= WIDTH || self.line as usize >= HEIGHT {
            return;
        }
        // STOP mode, or the blanked first frame after LCDC turns the LCD on:
        // every pixel is the same flat shade either way. See
        // `Gpu::set_stop_fill` and `Gpu::poweron_fill`.
        if let Some(shade) = self.stop_fill.or(self.poweron_fill) {
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

    /// Dots the fetcher spends painting the next scanline — the figure Pan Docs
    /// quotes as "mode 3 length", which is what every expectation below is.
    ///
    /// **It is one dot shorter than the window `STAT` reports mode 3 for**, and
    /// that is not an accounting slip: the mode bits flip at the top of the
    /// first mode-0 dot, so the dot that paints pixel 160 still reads mode 3
    /// when the CPU looks at it (`acceptance/ppu/lcdon_timing-GS` requires
    /// exactly that at dot 252). The loop below therefore leaves `modeclock`
    /// one dot past the last painted dot, and the `- 1` takes it back off.
    /// Measure the reported window instead and every figure here would read one
    /// higher while the fetcher had done identical work.
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
        gpu.modeclock - super::OAM_DOTS - 1
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

/// The two facts this engine learned from reading the emulators ahead of us on
/// the [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/) board —
/// docboy and SameBoy, both MIT/Expat, both read as hardware references and
/// reimplemented here per `CONTRIBUTING.md`. See
/// [`docs/mealybug.md`](../../docs/mealybug.md) §10.
#[cfg(test)]
mod fetcher_details {
    use super::super::{Gpu, RenderMode};
    use crate::mode::{GbMode, PpuRevision};

    /// A machine parked at the top of an ordinary scanline with the accurate
    /// engine selected.
    fn engine(mode: GbMode, rev: PpuRevision) -> Gpu {
        let mut gpu = Gpu::new();
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.gbmode = mode;
        gpu.set_cgb_ppu_revision(rev);
        gpu.wb(0xFF40, 0x93); // LCD on, BG on, OBJ on, tile data $8000
        gpu.do_cycle(super::LINE_DOTS); // past the line the LCD came up on
        gpu
    }

    /// **The dot an object is detected is still a fetcher dot.**
    ///
    /// docboy starts the object fetch from inside the background fetcher's own
    /// tick (`src/docboy/docboy/ppu/ppu.cpp`,
    /// `handle_pixel_slice_fetcher_push`: "Note: the first obj prefetcher tick
    /// overlaps this tick"), so the tile fetch in progress advances on that
    /// dot rather than freezing where it stood. Freezing it put every
    /// tile-number read after an object one dot late, which is precisely what
    /// `mealybug/m3_scx_high_5_bits` photographs.
    #[test]
    fn the_background_fetcher_still_gets_the_dot_an_object_is_detected() {
        let mut gpu = engine(GbMode::Classic, PpuRevision::CgbD);
        // One object at X = 16 — a tile boundary, so it stalls the output for
        // the full five-dot per-tile penalty and the fetcher is demonstrably
        // mid-fetch when it is detected. `engine` parks a few dots into the
        // OAM scan, so use an entry the scan has not walked past yet.
        for i in 0..40u16 {
            gpu.wb(0xFE00 + i * 4, 0);
            gpu.wb(0xFE00 + i * 4 + 1, 0);
        }
        let ly = gpu.rb(0xFF44);
        gpu.wb(0xFE00 + 10 * 4, 16 + ly);
        gpu.wb(0xFE00 + 10 * 4 + 1, 16);
        let mut seen = None;
        for _ in 0..super::LINE_DOTS {
            let before = gpu.pipe.fetch_t;
            let was_fetching = gpu.pipe.obj_t > 0;
            gpu.do_cycle(1);
            if !was_fetching && gpu.pipe.obj_t > 0 {
                seen = Some((before, gpu.pipe.fetch_t));
                break;
            }
        }
        let (before, after) = seen.expect("the object was never fetched");
        assert!(
            before < super::FETCH_DOTS,
            "the test needs the fetcher mid-tile when the object lands (was {before})"
        );
        assert_eq!(
            after,
            before + 1,
            "the background fetcher must advance on the dot the object is detected"
        );
    }

    /// Tile 0 with every row distinguishable: the low bitplane of row *r* is
    /// `r + 1`, the high bitplane `0x80 | (r + 1)`. Reading a fetched byte back
    /// therefore says which row the fetcher took it from.
    fn load_striped_tile(gpu: &mut Gpu) {
        for row in 0..8u16 {
            gpu.wb(0x8000 + row * 2, row as u8 + 1);
            gpu.wb(0x8000 + row * 2 + 1, 0x80 | (row as u8 + 1));
        }
    }

    /// **`SCY` is re-read at each bitplane fetch — except on a CPU CGB D.**
    ///
    /// SameBoy, `Core/display.c` (`GB_FETCHER_GET_TILE_T1`): "This value is
    /// cached on the CGB-D and newer, so it cannot be used to mix tiles
    /// together", with the bitplane fetch reading
    /// `gb->model > GB_MODEL_CGB_C ? gb->fetcher_y : fetcher_y(gb)`. docboy
    /// says the same thing from the other side (`bg_prefetcher_get_tile_0`:
    /// "on CGB there's no way to create a bitplane desync effect by changing
    /// SCY between the tile data low/high fetches"), and models D+ only.
    /// `mealybug/m3_scy_change`'s own two Color reference photographs differ by
    /// 6,217 pixels for exactly this reason.
    #[test]
    fn only_a_cgb_c_can_desync_the_two_bitplanes_with_scy() {
        for (mode, rev, desyncs) in [
            (GbMode::Classic, PpuRevision::CgbD, true),
            (GbMode::ColorAsClassic, PpuRevision::CgbC, true),
            (GbMode::ColorAsClassic, PpuRevision::CgbD, false),
        ] {
            let mut gpu = engine(mode, rev);
            load_striped_tile(&mut gpu);
            gpu.wb(0xFF42, 0); // SCY = 0: the fetcher is on tile row 0
            gpu.line = 0;
            gpu.fetch_tile_number();
            gpu.lo_hi(false);
            gpu.wb(0xFF42, 3); // ...and moves to row 3 mid-fetch
            gpu.lo_hi(true);
            assert_eq!(gpu.pipe.lo, 1, "the low bitplane is always row 0's");
            let want = if desyncs { 0x80 | 4 } else { 0x80 | 1 };
            assert_eq!(
                gpu.pipe.hi, want,
                "{mode:?}/{rev:?}: a CPU CGB D latches SCY + LY at the \
                 tile-number fetch; every other machine re-reads it"
            );
        }
    }

    /// **An object's row index is masked, never clamped.**
    ///
    /// `LCDC.2` is read at the bitplane fetch, not at the OAM scan, so the scan
    /// can accept an object as sixteen pixels tall and the fetch then find it
    /// eight — leaving a row index of 8-15 with only three bits to put it in.
    /// Hardware has no comparator there; the address lines drop the top bit and
    /// the fetch wraps to the top of the tile. SameBoy, `Core/display.c`,
    /// `get_object_line_address`:
    /// `tile_y = (gb->current_line - y) & (height_16? 0xF : 7)`.
    ///
    /// Clamping instead pins every one of those rows to row 7, which is what
    /// `mealybug/m3_lcdc_obj_size_change_scx` photographs: 22,154 pixels out on
    /// both Color columns (sixteen stray pixels of a shade hardware never
    /// produces there, which re-ranks the whole frame — docs/mealybug.md 8.8)
    /// and 298 on the DMG; 0/0/30 with the mask.
    #[test]
    fn an_objects_row_index_wraps_inside_the_tile_rather_than_clamping() {
        let mut gpu = engine(GbMode::Classic, PpuRevision::CgbD);
        // Tile $10, sixteen rows, each row's low bitplane holding its own
        // index so a fetched byte says which row it came from.
        for row in 0..16u16 {
            gpu.wb(0x8100 + row * 2, row as u8 + 1);
            gpu.wb(0x8100 + row * 2 + 1, 0);
        }
        // One object at Y = 16 (screen line 0), tile $10.
        for i in 0..40u16 {
            gpu.wb(0xFE00 + i * 4, 0);
        }
        gpu.wb(0xFE00, 16);
        gpu.wb(0xFE01, 8);
        gpu.wb(0xFE02, 0x10);
        gpu.wb(0xFE03, 0);
        gpu.pipe.scan[0] = super::ScanSprite {
            y: 16,
            x: 8,
            oam: 0,
        };
        gpu.pipe.nscan = 1;
        gpu.pipe.obj_i = 0;
        gpu.pipe.obj_tile = 0x10;
        gpu.pipe.obj_flags = 0;
        for (line, tall, want) in [
            (0u8, false, 1), // row 0
            (7, false, 8),   // row 7
            (9, false, 2),   // row 9 & 7 = 1 — NOT clamped to 7
            (13, false, 6),  // row 13 & 7 = 5
            (9, true, 10),   // sixteen tall: row 9 as it stands
            (13, true, 14),
        ] {
            gpu.line = line;
            gpu.wb(0xFF40, if tall { 0x97 } else { 0x93 });
            gpu.obj_plane(false);
            assert_eq!(
                gpu.pipe.obj_lo,
                want,
                "line {line}, {} tall",
                if tall { 16 } else { 8 }
            );
        }
        // Y-flip is the same mask, applied as an XOR.
        gpu.line = 0;
        gpu.wb(0xFF40, 0x93);
        gpu.pipe.obj_flags = 0x40;
        gpu.obj_plane(false);
        assert_eq!(gpu.pipe.obj_lo, 8, "flipped row 0 of eight is row 7");
    }

    /// The knob is spelled the way the APU's already is, so one
    /// `$GB_CGB_REVISION` configures both — and a tag naming several steppings
    /// resolves to the one this type distinguishes.
    #[test]
    fn the_revision_tags_land_on_the_right_side_of_the_boundary() {
        for tag in ["cgb0", "cgbB", "cgbC", "cgb0BC", "C"] {
            assert_eq!(PpuRevision::from_tag(tag), Some(PpuRevision::CgbC), "{tag}");
        }
        for tag in ["cgbD", "D"] {
            assert_eq!(PpuRevision::from_tag(tag), Some(PpuRevision::CgbD), "{tag}");
        }
        // `cgbDE` means "D and E agree", and for everything this type decides
        // except the `TILE_SEL` glitch they do. It resolves to the machine this
        // emulator claims to be.
        for tag in ["cgbDE", "cgbE", "e"] {
            assert_eq!(PpuRevision::from_tag(tag), Some(PpuRevision::CgbE), "{tag}");
        }
        assert_eq!(PpuRevision::from_tag("agb"), None);
        assert_eq!(PpuRevision::default(), PpuRevision::CgbE);
        // The three predicates and where each boundary falls: `CgbD` and
        // `CgbE` are one machine everywhere but the glitch, which is the whole
        // reason they are separate values.
        for r in [PpuRevision::CgbD, PpuRevision::CgbE] {
            assert!(r.caches_fetcher_y(), "{r:?}");
            assert!(r.late_palette_commit(), "{r:?}");
        }
        assert!(!PpuRevision::CgbC.caches_fetcher_y());
        assert!(!PpuRevision::CgbC.late_palette_commit());
        assert!(PpuRevision::CgbC.tile_sel_glitch_reads_tile_index());
        assert!(PpuRevision::CgbE.tile_sel_glitch_reads_tile_index());
        assert!(!PpuRevision::CgbD.tile_sel_glitch_reads_tile_index());
    }
}

/// The head of a scanline: the `SCX & 7` discard, and what a `WX = 0` window
/// does to it.
///
/// Both behaviours below are `mealybug/m3_window_timing_wx_0`'s subject, stated
/// in that ROM's own header: *"The stair pattern is visible due to the delay
/// from the lowest 3 bits of SCX, and due to window activating one T-cycle
/// later when WX = 0 and SCX > 0."* They are pinned here at unit speed because
/// the ROM can only show them as a picture.
#[cfg(test)]
mod head_of_line {
    use super::super::{Gpu, RenderMode};

    /// A DMG with the accurate engine, parked at the top of an ordinary
    /// scanline (past the line the LCD came up on, which has no OAM scan).
    fn engine() -> Gpu {
        let mut gpu = Gpu::new();
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.wb(0xFF40, 0x91); // LCD on, BG on, tile data $8000
        gpu.do_cycle(super::LINE_DOTS);
        gpu
    }

    /// Step to the first dot of mode 3, then run `f` at each dot until the
    /// first pixel is painted. Returns that dot, counted from mode 3's start.
    fn first_pixel_dot(gpu: &mut Gpu, mut f: impl FnMut(&mut Gpu, u32)) -> u32 {
        while gpu.mode() != 2 {
            gpu.do_cycle(1);
        }
        while gpu.mode() != 3 {
            gpu.do_cycle(1);
        }
        for _ in 0..400 {
            let dot = gpu.modeclock - super::OAM_DOTS;
            f(gpu, dot);
            gpu.do_cycle(1);
            if gpu.pipe.lx > 0 {
                return dot;
            }
        }
        panic!("no pixel was ever painted");
    }

    /// With nothing moving, the first pixel lands on dot `12 + (SCX & 7)`: the
    /// twelve dots of the discarded first fetch, then one dot per pixel thrown
    /// away.
    #[test]
    fn the_first_pixel_waits_for_the_prologue_and_the_discards() {
        for scx in 0..16u8 {
            let mut gpu = engine();
            gpu.wb(0xFF43, scx);
            assert_eq!(
                first_pixel_dot(&mut gpu, |_, _| {}),
                12 + (scx & 7) as u32,
                "SCX={scx}"
            );
        }
    }

    /// **`SCX & 7` is re-read on every discarded dot, not latched when mode 3
    /// began.** Every Mealybug handler writes `SCX` around dot 8 — five
    /// M-cycles of interrupt dispatch and a jump get it no earlier — and the
    /// first pixel is not shifted until dot 12, so a latch at dot 0 uses the
    /// *previous* line's value and the whole stair pattern comes out one line
    /// stale.
    #[test]
    fn a_scx_write_before_the_first_pixel_still_decides_the_discard() {
        for scx in 0..8u8 {
            let mut gpu = engine();
            gpu.wb(0xFF43, 7); // whatever the previous line left behind
            let dot = first_pixel_dot(&mut gpu, |g, dot| {
                if dot == 8 {
                    g.wb(0xFF43, scx);
                }
            });
            assert_eq!(dot, 12 + scx as u32, "SCX written at dot 8 as {scx}");
        }
    }

    /// A write that lands *after* the discards have begun moves the target the
    /// counter is running at, and the counter wraps at 8 — so lowering `SCX`
    /// mid-discard costs a further lap rather than ending the phase early.
    /// SameBoy reaches the same place from the other side, comparing
    /// `(position_in_line & 7)` against the live register
    /// (`Core/display.c`, `render_pixel_if_possible`).
    #[test]
    fn lowering_scx_mid_discard_costs_a_whole_lap() {
        let mut gpu = engine();
        gpu.wb(0xFF43, 5);
        // Dots 12,13,14 drop three pixels; at dot 15 the count is 3 and the
        // target becomes 1, which it can only reach by wrapping through 8.
        let dot = first_pixel_dot(&mut gpu, |g, dot| {
            if dot == 15 {
                g.wb(0xFF43, 1);
            }
        });
        assert_eq!(dot, 12 + 3 + (8 - 3 + 1) as u32);
    }

    /// **`WX = 0` costs one extra dot when the line has fractional scrolling.**
    /// The window's takeover spends that dot and the fetcher does not advance
    /// on it. SameBoy charges it in the same place — `Core/display.c`,
    /// immediately after `fifo_clear(&gb->bg_fifo)`:
    /// `if (WX == 0 && (SCX & 7) …) { cycles_for_line += 1; GB_SLEEP(…, 1); }`.
    #[test]
    fn a_wx_0_window_costs_an_extra_dot_only_with_fractional_scrolling() {
        for scx in 0..8u8 {
            let mut gpu = engine();
            gpu.wb(0xFF43, scx);
            gpu.wb(0xFF4A, 0); // WY = 0: the Y condition latches at once
            gpu.wb(0xFF4B, 0); // WX = 0
            gpu.wb(0xFF40, 0xB1); // + LCDC.5, the window
            let extra = u32::from(scx & 7 != 0);
            assert_eq!(
                first_pixel_dot(&mut gpu, |_, _| {}),
                // 12 dots of prologue, the window's own 6-dot startup fetch,
                // then the discards — plus the one extra dot.
                18 + (scx & 7) as u32 + extra,
                "SCX={scx}"
            );
        }
    }

    /// **An object's `X` is a screen coordinate, so the window's off-screen
    /// pixels must not consume it.** With `WX < 7` the window's first tile
    /// starts left of the screen and `7 - WX` of its pixels are dropped; if the
    /// sprite FIFO is shifted against them, an object at the left edge loses
    /// exactly that many of its own. `mealybug/m3_wx_4_change_sprites` tiles
    /// its whole line with objects at `X = 8, 16, 24 …` over a `WX = 4` window
    /// and is 24 pixels — three on each of eight lines — out when it is.
    #[test]
    fn the_windows_off_screen_pixels_do_not_eat_an_object_at_the_left_edge() {
        const LINE: u8 = 3;
        let mut gpu = engine();
        // A solid tile for both the background and the object.
        for b in 0..16u16 {
            gpu.wb(0x8010 + b, 0xFF);
        }
        for t in 0..32u16 {
            gpu.wb(0x9800 + t, 1);
            gpu.wb(0x9C00 + t, 1);
        }
        gpu.wb(0xFF47, 0xE4); // BGP identity: background/window are colour 3
        gpu.wb(0xFF48, 0x40); // OBP0: object colour 3 -> shade 1, distinct
        for i in 0..40u16 {
            gpu.wb(0xFE00 + i * 4, 0);
        }
        gpu.wb(0xFE00, 16 + LINE); // Y: on LINE
        gpu.wb(0xFE01, 8); // X = 8 -> screen columns 0..7
        gpu.wb(0xFE02, 1);
        gpu.wb(0xFE03, 0);
        gpu.wb(0xFF4A, LINE); // WY
        gpu.wb(0xFF4B, 4); // WX = 4: three window pixels are off-screen
        gpu.wb(0xFF40, 0xB3); // LCD, BG, OBJ, window all on
        let mut seen = 0;
        while seen < 2 {
            let before = gpu.line;
            gpu.do_cycle(1);
            if before == 143 && gpu.line == 144 {
                seen += 1;
            }
        }
        let row = |x: usize| gpu.data[(LINE as usize * super::super::WIDTH + x) * 4];
        let obj = row(0);
        for x in 0..8 {
            assert_eq!(
                row(x),
                obj,
                "column {x} of the object at X = 8 is missing (the window's \
                 off-screen pixels ate it)"
            );
        }
    }

    /// **The window pixel-insertion glitch.** Once `WY == LY` has latched for
    /// the frame, a background push landing on the pixel the window starts at
    /// supplies one transparent pixel instead of the tile and postpones the
    /// real push by a dot — so everything from there rightwards is one pixel
    /// over. It does **not** need the window to be drawing, and the window's own
    /// first tile is exempt.
    ///
    /// docboy, `src/docboy/docboy/ppu/ppu.cpp`,
    /// `handle_pixel_slice_fetcher_push`: "if this push stage happens when
    /// LX == WX, then a 00 pixel is pushed into the bg fifo instead of the
    /// fetched tile … The first window tile after the window activation seems
    /// to be an exception." SameBoy reaches the same place through
    /// `gb->insert_bg_pixel` (`Core/display.c`, citing its issue #278).
    #[test]
    fn a_push_on_the_windows_own_start_pixel_inserts_one_transparent_pixel() {
        const LINE: u8 = 3;
        // Tile 1 is solid colour 3 and the whole background map uses it, so an
        // inserted transparent pixel shows up as a hole in a flat line.
        let mut gpu = engine();
        for b in 0..16u16 {
            gpu.wb(0x8010 + b, 0xFF);
        }
        for t in 0..32u16 {
            gpu.wb(0x9800 + t, 1);
        }
        gpu.wb(0xFF47, 0xE4); // BGP identity
        gpu.wb(0xFF4A, LINE); // the Y condition latches on this line...
                              // ...and the window would start at pixel 8. A push only ever lands on
                              // the eight-pixel grid, so `WX - 7` has to be on it for the glitch to
                              // be reachable at all — which is exactly why `m3_wx_4_change` shows it
                              // on one line in eight.
        gpu.wb(0xFF4B, 8 + 7);
        gpu.wb(0xFF40, 0x91); // LCDC.5 is off, so the window never draws.
                              // Run a whole frame so the line is drawn with the Y condition latched.
        let mut seen = 0;
        while seen < 2 {
            let before = gpu.line;
            gpu.do_cycle(1);
            if before == 143 && gpu.line == 144 {
                seen += 1;
            }
        }
        let row = |x: usize| gpu.data[(LINE as usize * super::super::WIDTH + x) * 4];
        let solid = row(0);
        let hole = (0..super::super::WIDTH).find(|&x| row(x) != solid);
        assert_eq!(
            hole,
            Some(8),
            "a transparent pixel should have been inserted at WX - 7"
        );
        assert_eq!(row(9), solid, "and only one of them");
    }

    /// **…and on Color silicon it does not fire unless the window is drawing.**
    ///
    /// The same program on a Color console, which is the `cgb-acid2` case:
    /// `LCDC.5` is off, the Y condition has latched, and a push lands exactly
    /// on `WX - 7`. docboy gates the glitch `#ifdef ENABLE_CGB && w.active`
    /// (`handle_pixel_slice_fetcher_push`, "on CGB the glitch seems to happen
    /// only if we are fetching a window tile right now"), so the line must come
    /// out whole. Ungated it inserts a pixel here and every column right of 8
    /// is one to the right — 158 pixels of `cgb-acid2`, and 61 rows of
    /// Gambatte's `window` group.
    ///
    /// **A Color machine ignores `BGP`,** so the palette has to be written
    /// through `BCPS`/`BCPD` or colour 0 and colour 3 come out of an all-zero
    /// CGB palette as the same colour and the hole is invisible — which is how
    /// the first version of this test passed with the gate removed.
    ///
    /// Teeth: drop the `is_dmg() || pipe.window` clause and this fails at
    /// column 8 while the DMG case above keeps passing.
    #[test]
    fn a_colour_console_needs_the_window_drawing_for_that_glitch() {
        const LINE: u8 = 3;
        let mut gpu = Gpu::new();
        gpu.set_render_mode(RenderMode::Accurate);
        gpu.gbmode = crate::mode::GbMode::Color;
        gpu.wb(0xFF40, 0x91);
        gpu.do_cycle(super::super::LINE_DOTS);
        for b in 0..16u16 {
            gpu.wb(0x8010 + b, 0xFF);
        }
        for t in 0..32u16 {
            gpu.wb(0x9800 + t, 1);
        }
        // Background palette 0: colour 0 black, colour 3 white.
        gpu.wb(0xFF68, 0x80); // BCPS: index 0, auto-increment
        for c in 0..4u16 {
            let v: u16 = if c == 3 { 0x7FFF } else { 0x0000 };
            gpu.wb(0xFF69, (v & 0xFF) as u8);
            gpu.wb(0xFF69, (v >> 8) as u8);
        }
        gpu.wb(0xFF4A, LINE);
        gpu.wb(0xFF4B, 8 + 7);
        gpu.wb(0xFF40, 0x91); // window still off
        let mut seen = 0;
        while seen < 2 {
            let before = gpu.line;
            gpu.do_cycle(1);
            if before == 143 && gpu.line == 144 {
                seen += 1;
            }
        }
        let row = |x: usize| gpu.data[(LINE as usize * super::super::WIDTH + x) * 4];
        let solid = row(0);
        assert_ne!(
            solid, 0,
            "the fixture must draw a non-black line to measure"
        );
        let hole = (0..super::super::WIDTH).find(|&x| row(x) != solid);
        assert_eq!(
            hole, None,
            "a Color console must not insert a pixel while the window is off"
        );
    }

    /// **The window's tile grid is anchored at `WX - 7`, even when that is off
    /// the left edge of the screen.**
    ///
    /// Ground truth is `mealybug/m3_lcdc_win_en_change_multiple_wx@DMG`, which
    /// sets `WX = LY` and takes `LCDC.5` low a few pixels into every line: the
    /// background resumes at **9, 10, 3, 4, 5, 6, 7, 8** for `WX = 0..7`, which
    /// is exactly the first `WX - 7 + 8n` at or after the write. Anchoring the
    /// grid at `max(0, WX - 7)` instead resumes at 16 on every one of them.
    ///
    /// The window therefore takes over **inside the head-of-line prologue** for
    /// `WX <= 6`, and its first tile's off-screen pixels are shifted out and
    /// dropped a dot each — which is what keeps pixel 0 on dot 18 whatever `WX`
    /// is (see `a_wx_0_window_costs_an_extra_dot_only_with_fractional_scrolling`
    /// and `m3_window_timing`, both unmoved by this).
    #[test]
    fn the_window_tile_grid_is_anchored_at_wx_minus_7() {
        for wx in 0..=12u8 {
            let mut gpu = engine();
            gpu.wb(0xFF4A, 0); // WY = 0
            gpu.wb(0xFF4B, wx);
            gpu.wb(0xFF40, 0xB1); // + LCDC.5
            while gpu.mode() != 2 {
                gpu.do_cycle(1);
            }
            while gpu.mode() != 3 {
                gpu.do_cycle(1);
            }
            // Let the window take over and paint its first visible pixel, then
            // withdraw `LCDC.5`. The background resumes at the end of the
            // window tile being drawn, so where it resumes *is* the grid.
            while !gpu.pipe.window {
                gpu.do_cycle(1);
                assert_eq!(gpu.mode(), 3, "WX={wx}: the window never started");
            }
            while gpu.pipe.lx == 0 {
                gpu.do_cycle(1);
            }
            gpu.wb(0xFF40, 0x91); // LCDC.5 off
            while gpu.pipe.window {
                gpu.do_cycle(1);
                assert_eq!(gpu.mode(), 3, "WX={wx}: the window never stopped");
            }
            let resume = gpu.pipe.lx as i16;
            let start = wx as i16 - 7;
            assert_eq!(
                (resume - start).rem_euclid(8),
                0,
                "WX={wx}: the background resumed at {resume}, which is not on \
                 the window's own tile grid ({start} + 8n)"
            );
        }
    }

    /// **The window starts on the pixel `WX - 7` and on no other.** Re-enabling
    /// `LCDC.5` later in the same scanline is therefore a no-op unless `WX` has
    /// been moved somewhere the output has not reached yet — mealybug's own
    /// notes, WIN_EN: "Setting WIN_EN again during mode 3 on the same scanline
    /// will have no effect unless WX has been updated to set the window to
    /// activate on a pixel that hasn't been drawn yet."
    #[test]
    fn a_window_re_enabled_behind_the_output_does_not_start_again() {
        let mut gpu = engine();
        gpu.wb(0xFF4A, 0); // WY = 0
        gpu.wb(0xFF4B, 40); // the window starts at pixel 33
        gpu.wb(0xFF40, 0xB1); // + LCDC.5
                              // Run until the window has taken over and drawn a whole tile, then
                              // turn it off. It resumes the background at the next tile boundary.
        while !gpu.pipe.window {
            gpu.do_cycle(1);
        }
        while gpu.pipe.lx < 48 {
            gpu.do_cycle(1);
        }
        gpu.wb(0xFF40, 0x91); // LCDC.5 off
        while gpu.pipe.window {
            gpu.do_cycle(1);
        }
        let resumed_at = gpu.pipe.lx;
        // Now turn it straight back on, leaving `WX` where it was — behind the
        // output. Nothing may happen for the rest of the line.
        gpu.wb(0xFF40, 0xB1);
        while gpu.mode() == 3 {
            gpu.do_cycle(1);
            assert!(
                !gpu.pipe.window,
                "the window restarted at lx={} with WX-7={} already drawn",
                gpu.pipe.lx, 33
            );
        }
        assert!(resumed_at >= 48, "the background never resumed");
    }

    /// ...and moving `WX` ahead of the output first *does* start it again.
    #[test]
    fn a_window_re_enabled_ahead_of_the_output_starts_again() {
        let mut gpu = engine();
        gpu.wb(0xFF4A, 0);
        gpu.wb(0xFF4B, 40);
        gpu.wb(0xFF40, 0xB1);
        while !gpu.pipe.window {
            gpu.do_cycle(1);
        }
        while gpu.pipe.lx < 48 {
            gpu.do_cycle(1);
        }
        gpu.wb(0xFF40, 0x91);
        while gpu.pipe.window {
            gpu.do_cycle(1);
        }
        gpu.wb(0xFF4B, 120); // pixel 113, still ahead of the output
        gpu.wb(0xFF40, 0xB1);
        let mut restarted = false;
        while gpu.mode() == 3 {
            gpu.do_cycle(1);
            if gpu.pipe.window {
                restarted = true;
                break;
            }
        }
        assert!(restarted, "the window did not restart at the new WX");
    }

    /// The extra dot is `WX = 0`'s alone: at any other `WX` the window's
    /// startup costs its six dots and nothing more.
    #[test]
    fn no_other_window_position_pays_the_extra_dot() {
        // `WX <= 7` puts the window's left edge at or before pixel 0, so it
        // takes the fetcher over before anything is painted and its own 6-dot
        // startup is all it costs.
        for wx in [1u8, 7] {
            let mut gpu = engine();
            gpu.wb(0xFF43, 3);
            gpu.wb(0xFF4A, 0);
            gpu.wb(0xFF4B, wx);
            gpu.wb(0xFF40, 0xB1);
            assert_eq!(first_pixel_dot(&mut gpu, |_, _| {}), 18 + 3, "WX={wx}");
        }
        // `WX = 8` starts the window at pixel 1, so pixel 0 is a background
        // pixel and lands on the plain `12 + (SCX & 7)`.
        let mut gpu = engine();
        gpu.wb(0xFF43, 3);
        gpu.wb(0xFF4A, 0);
        gpu.wb(0xFF4B, 8);
        gpu.wb(0xFF40, 0xB1);
        assert_eq!(first_pixel_dot(&mut gpu, |_, _| {}), 12 + 3);
    }
}

/// A dot-by-dot trace of one Mealybug ROM, for the campaign that closes the
/// rest of them. Ignored by default; it needs a ROM this repository does not
/// ship (see `tools/fetch-test-suites.sh`) and prints rather than asserts.
///
/// ```text
/// GB_PPU=identical SCRATCH_ROM=target/mealybug/m3_scy_change.gb SCRATCH_LINE=8 \
///     cargo test --release --lib -- --ignored --nocapture mid_line_trace
/// ```
///
/// Every mid-scanline row comes down to *which dot* a register write lands on
/// against *which dot* a pixel is shifted out, and no picture can show that.
#[cfg(test)]
mod tracing {
    use crate::cpu::core::Cpu;
    use crate::mode::Model;

    #[test]
    #[ignore = "needs a Mealybug ROM and prints rather than asserts"]
    fn mid_line_trace() {
        let Ok(path) = std::env::var("SCRATCH_ROM") else {
            return;
        };
        let want: u8 = std::env::var("SCRATCH_LINE")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(8);
        let model = match std::env::var("SCRATCH_MODEL").as_deref() {
            Ok("cgb") => Model::Cgb,
            _ => Model::Dmg,
        };
        let data = std::fs::read(&path).expect("read the ROM named by SCRATCH_ROM");
        let mut cpu = Cpu::new_with_model(data, None, model);
        let (mut frames, mut logged) = (0u32, 0);
        for _ in 0..40_000_000u64 {
            let before = cpu.memory.gpu.line;
            cpu.do_cycle();
            let g = &cpu.memory.gpu;
            if before == 143 && g.line == 144 {
                frames += 1;
            }
            if frames >= 8 && g.line == want && logged < 120 {
                println!(
                    "dot={:4} mode={} lx={:3} scx={:3} scy={:3} wx={:3} \
                     lcdc0={} win={} winact={} tilewin={} win_on={} fetch_x={:3} \
                     win_fetch_x={:3} fetch_t={} win_skip={} bgp={:?}",
                    g.modeclock,
                    g.mode(),
                    g.pipe.lx,
                    g.scx,
                    g.scy,
                    g.winx,
                    g.lcdc0,
                    g.pipe.window,
                    g.pipe.win_active,
                    g.pipe.tile_is_window,
                    g.win_on,
                    g.pipe.fetch_x,
                    g.pipe.win_fetch_x,
                    g.pipe.fetch_t,
                    g.pipe.win_skip,
                    g.palb,
                );
                logged += 1;
            }
            if frames >= 9 {
                break;
            }
        }
    }
}

/// Proves the version-gated defaults in [`Pipeline::load_state`] against
/// genuinely shorter old-format byte streams — built with
/// [`Pipeline::save_state_as_of`], not by stamping current bytes with an old
/// version number, so these exercise real byte-count handling and not just
/// the `if` conditions in isolation. See `crate::savestate::
/// MIN_SUPPORTED_VERSION` for the whole compatibility contract this task
/// implements, and `AGENTS.md`'s save-state section for how the field-level
/// defaults were chosen.
#[cfg(test)]
mod savestate_compat {
    use super::Pipeline;
    use crate::savestate::{Reader, Writer};

    /// A pre-v24 blob upgrades so that `win_active`/`tile_is_window` mirror
    /// the just-loaded `window` (what `fetch_tile_number`/`lo_hi` read
    /// directly before the split), `win_fetch_x` mirrors `fetch_x` (the one
    /// shared counter both fetchers used before the split), and
    /// `line_started` reconstructs the exact `bg_len > 0` inference the flag
    /// replaced. A trailing marker byte proves the reader consumes precisely
    /// this blob's own length and nothing of whatever follows it in a real
    /// save state.
    #[test]
    fn a_pre_v24_pipeline_upgrades_and_consumes_exactly_its_own_bytes() {
        // Distinguishing, non-default values: a defaulted field reading back
        // as its own zero-value default could otherwise hide a bug where the
        // gate never ran at all.
        let p = Pipeline {
            window: true,
            fetch_x: 5,
            bg_len: 3,
            ..Pipeline::default()
        };

        for version in [22u16, 23] {
            let mut w = Writer::new();
            p.save_state_as_of(&mut w, version);
            const MARKER: u8 = 0xA5;
            w.u8(MARKER);

            let mut r = Reader::new(&w.buf);
            let mut loaded = Pipeline::default();
            loaded
                .load_state(&mut r, version)
                .unwrap_or_else(|e| panic!("v{version} pipeline state should load: {e}"));

            assert!(loaded.window, "v{version}");
            assert_eq!(loaded.fetch_x, 5, "v{version}");
            assert_eq!(loaded.bg_len, 3, "v{version}");
            assert!(loaded.win_active, "v{version}: defaults to `window`");
            assert!(loaded.tile_is_window, "v{version}: defaults to `window`");
            assert_eq!(loaded.win_fetch_x, 5, "v{version}: defaults to `fetch_x`");
            assert!(loaded.line_started, "v{version}: bg_len > 0");

            assert_eq!(
                r.u8().unwrap(),
                MARKER,
                "v{version}: load_state did not consume exactly this blob's own length"
            );
        }
    }

    /// The other direction: `window` false and an empty FIFO must default
    /// `win_active`/`tile_is_window`/`line_started` to false too, not to
    /// whatever `Pipeline::default()` happened to leave lying around (which
    /// would pass by coincidence if the gate defaulted blindly to `false`
    /// rather than actually reading `window`/`bg_len`).
    #[test]
    fn a_pre_v24_pipeline_with_no_window_and_an_empty_fifo_defaults_the_other_way() {
        let p = Pipeline::default();
        let mut w = Writer::new();
        p.save_state_as_of(&mut w, 23);

        let mut r = Reader::new(&w.buf);
        let mut loaded = Pipeline::default();
        loaded
            .load_state(&mut r, 23)
            .expect("v23 pipeline state should load");

        assert!(!loaded.win_active);
        assert!(!loaded.tile_is_window);
        assert_eq!(loaded.win_fetch_x, 0);
        assert!(!loaded.line_started, "bg_len == 0 -> not yet pushed");
    }

    /// v24 and later already carry the real fields — `save_state_as_of`
    /// writing the same bytes as the production writer at that version is
    /// what makes it a faithful stand-in for one, not a shortcut.
    #[test]
    fn a_v24_pipeline_round_trips_its_real_fields_unchanged() {
        let p = Pipeline {
            win_active: true,
            tile_is_window: false,
            win_fetch_x: 9,
            line_started: true,
            ..Pipeline::default()
        };

        let mut w = Writer::new();
        p.save_state_as_of(&mut w, 24);
        let mut w_prod = Writer::new();
        p.save_state(&mut w_prod);
        assert_eq!(
            w.buf, w_prod.buf,
            "save_state_as_of(.., 24) must match the real production writer"
        );

        let mut r = Reader::new(&w.buf);
        let mut loaded = Pipeline::default();
        loaded
            .load_state(&mut r, 24)
            .expect("v24 pipeline state should load");
        assert!(loaded.win_active);
        assert!(!loaded.tile_is_window);
        assert_eq!(loaded.win_fetch_x, 9);
        assert!(loaded.line_started);
    }
}
