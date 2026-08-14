use crate::mode::GbMode;
use std::cmp::Ordering;

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
    hblanking: bool,
    /// The current level of the **STAT interrupt line** — the OR of every
    /// enabled STAT source. `IF.1` is raised on its 0→1 transition only; see
    /// [`Gpu::refresh_stat_line`].
    stat_line: bool,
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
            cbgpal_inc: false,
            cbgpal_ind: 0,
            cbgpal: [[[0u8; 3]; 4]; 8],
            csprit_inc: false,
            csprit_ind: 0,
            csprit: [[[0u8; 3]; 4]; 8],
            vrambank: 0,
            hblanking: false,
            stat_line: false,
            lyc_match: false,
            skip_first_oam: false,
            stop_fill: None,
        }
    }

    /// The current STAT mode (0 HBlank, 1 VBlank, 2 OAM scan, 3 drawing).
    pub fn mode(&self) -> u8 {
        self.mode
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

    /// Serialise the PPU. Everything that drives future rendering is captured,
    /// including the internal `modeclock`/`line` counters and the CGB palette
    /// memory. `bgprio` is *not* saved: it is fully rewritten at the start of
    /// every `renderscan` before being read, so it holds no state across
    /// frames. The `data` framebuffer *is* saved so the visible frame is
    /// correct the instant a state loads.
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
        w.bool(self.lyc_match);
        w.bool(self.skip_first_oam);
        // STOP mode can be live at a frame boundary (the CPU sits there until
        // a button is pressed), so the blanked display has to survive a state.
        w.bool(self.stop_fill.is_some());
        w.u8(self.stop_fill.unwrap_or(0));
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
        self.lyc_match = r.bool()?;
        self.skip_first_oam = r.bool()?;
        let stopped = r.bool()?;
        let fill = r.u8()?;
        self.stop_fill = if stopped { Some(fill) } else { None };
        Ok(())
    }

    // The PPU mode cycle. See docs/components/ppu.md §"The mode cycle" and Pan Docs:
    // https://gbdev.io/pandocs/Rendering.html  (STAT modes: https://gbdev.io/pandocs/STAT.html)
    pub fn do_cycle(&mut self, ticks: u32) {
        if !self.lcd_on {
            return;
        }
        self.hblanking = false;

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
        (self.lyc_inte && self.lyc_match)
            || (self.lcd_on
                && ((self.m0_inte && self.mode == 0)
                    || (self.m1_inte && self.mode == 1)
                    || (self.m2_inte && self.oam_strobe())))
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
    fn oam_strobe(&self) -> bool {
        self.mode == 2 || (self.line == VBLANK_LINE && self.modeclock < OAM_DOTS)
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
        let now = self.stat_signal();
        if now && !self.stat_line {
            self.interrupt |= 0x02;
        }
        self.stat_line = now;
    }

    fn change_mode(&mut self, mode: u8) {
        self.mode = mode;

        match self.mode {
            0 => {
                self.renderscan();
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
                    | (if self.lyc_match { 0x04 } else { 0 })
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
            0xFF4F..=0xFF6B if self.gbmode != GbMode::Color => 0xFF,
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
        // STOP mode: the PPU has been cut off from VRAM, so it draws a flat
        // shade rather than the tile map — see `stop_fill`.
        if let Some(shade) = self.stop_fill {
            for x in 0..WIDTH {
                self.setcolor(x, shade);
                self.bgprio[x] = PrioType::Normal;
            }
            return;
        }
        for x in 0..WIDTH {
            self.setcolor(x, 255);
            self.bgprio[x] = PrioType::Normal;
        }
        self.draw_bg();
        self.draw_sprites();
    }

    fn setcolor(&mut self, x: usize, color: u8) {
        self.data[self.line as usize * WIDTH * 4 + x * 4] = color;
        self.data[self.line as usize * WIDTH * 4 + x * 4 + 1] = color;
        self.data[self.line as usize * WIDTH * 4 + x * 4 + 2] = color;
    }

    fn setrgb(&mut self, x: usize, r: u8, g: u8, b: u8) {
        // Gameboy Color RGB correction
        // Taken from the Gambatte emulator
        // assume r, g and b are between 0 and 1F
        let baseidx = self.line as usize * WIDTH * 4 + x * 4;

        let r = r as u32;
        let g = g as u32;
        let b = b as u32;

        self.data[baseidx] = ((r * 13 + g * 2 + b) >> 1) as u8;
        self.data[baseidx + 1] = ((g * 3 + b) << 1) as u8;
        self.data[baseidx + 2] = ((r * 3 + g * 2 + b * 11) >> 1) as u8;
    }

    fn draw_bg(&mut self) {
        let drawbg = self.gbmode == GbMode::Color || self.lcdc0;

        let wx_trigger = self.winx <= 166;
        let winy = if self.win_on && self.wy_trigger && wx_trigger {
            self.wy_pos += 1;
            self.wy_pos
        } else {
            -1
        };

        if winy < 0 && !drawbg {
            return;
        }

        let wintiley = (winy as u16 >> 3) & 31;

        let bgy = self.scy.wrapping_add(self.line);
        let bgtiley = (bgy as u16 >> 3) & 31;

        for x in 0..WIDTH {
            let winx = -((self.winx as i32) - 7) + (x as i32);
            let bgx = self.scx as u32 + x as u32;

            let (tilemapbase, tiley, tilex, pixely, pixelx) = if winy >= 0 && winx >= 0 {
                (
                    self.win_tilemap,
                    wintiley,
                    (winx as u16 >> 3),
                    winy as u16 & 0x07,
                    winx as u8 & 0x07,
                )
            } else if drawbg {
                (
                    self.bg_tilemap,
                    bgtiley,
                    (bgx as u16 >> 3) & 31,
                    bgy as u16 & 0x07,
                    bgx as u8 & 0x07,
                )
            } else {
                continue;
            };

            let tilenr: u8 = self.rbvram0(tilemapbase + tiley * 32 + tilex);

            let (palnr, vram1, xflip, yflip, prio) = if self.gbmode == GbMode::Color {
                let flags = self.rbvram1(tilemapbase + tiley * 32 + tilex) as usize;
                (
                    flags & 0x07,
                    flags & (1 << 3) != 0,
                    flags & (1 << 5) != 0,
                    flags & (1 << 6) != 0,
                    flags & (1 << 7) != 0,
                )
            } else {
                (0, false, false, false, false)
            };

            let tileaddress = self.tilebase
                + (if self.tilebase == 0x8000 {
                    tilenr as u16
                } else {
                    (tilenr as i8 as i16 + 128) as u16
                }) * 16;

            let a0 = match yflip {
                false => tileaddress + (pixely * 2),
                true => tileaddress + (14 - (pixely * 2)),
            };

            let (b1, b2) = match vram1 {
                false => (self.rbvram0(a0), self.rbvram0(a0 + 1)),
                true => (self.rbvram1(a0), self.rbvram1(a0 + 1)),
            };

            let xbit = match xflip {
                true => pixelx,
                false => 7 - pixelx,
            } as u32;
            let colnr = if b1 & (1 << xbit) != 0 { 1 } else { 0 }
                | if b2 & (1 << xbit) != 0 { 2 } else { 0 };

            self.bgprio[x] = if colnr == 0 {
                PrioType::Color0
            } else if prio {
                PrioType::PrioFlag
            } else {
                PrioType::Normal
            };
            if self.gbmode == GbMode::Color {
                let r = self.cbgpal[palnr][colnr][0];
                let g = self.cbgpal[palnr][colnr][1];
                let b = self.cbgpal[palnr][colnr][2];
                self.setrgb(x, r, g, b);
            } else {
                let color = self.palb[colnr];
                self.setcolor(x, color);
            }
        }
    }

    fn draw_sprites(&mut self) {
        if !self.sprite_on {
            return;
        }

        let line = self.line as i32;
        let sprite_size = self.sprite_size as i32;

        let mut sprites_to_draw = [(0, 0, 0); 10];
        let mut sidx = 0;
        for index in 0..40 {
            let spriteaddr = 0xFE00 + (index as u16) * 4;
            let spritey = self.rb(spriteaddr) as u16 as i32 - 16;
            if line < spritey || line >= spritey + sprite_size {
                continue;
            }
            let spritex = self.rb(spriteaddr + 1) as u16 as i32 - 8;
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

            let spriteaddr = 0xFE00 + (i as u16) * 4;
            let tilenum = (self.rb(spriteaddr + 2)
                & (if self.sprite_size == 16 { 0xFE } else { 0xFF }))
                as u16;
            let flags = self.rb(spriteaddr + 3) as usize;
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
    pub fn may_hdma(&self) -> bool {
        self.hblanking
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
}
