//! Cover art, taken from the game itself.
//!
//! There is no artwork in a cartridge header — but there is artwork *in the
//! game*, and a title screen is the closest thing a Game Boy has to an album
//! sleeve. So: boot the ROM, watch the frames go by, and keep the ones that look
//! like a title screen.
//!
//! **Several candidates, not one.** Titles animate in — a publisher logo, a
//! character walking on, a flash, then the title. Any single moment is a guess,
//! and a wrong guess is a shelf of grey rectangles. The picker therefore keeps
//! the best few *distinct* frames and the shelf lets the user cycle them, which
//! turns "we picked wrong" from a defect into a keypress.
//!
//! Nothing here is written into this repository. Covers are produced at run time
//! from a ROM the user already owns; see [`crate::music`].

use crate::gpu::{HEIGHT, WIDTH};

/// One candidate cover: a whole 160x144 RGBA frame plus where it came from.
#[derive(Clone, Default)]
pub struct Cover {
    /// RGBA, `WIDTH * HEIGHT * 4` bytes, exactly as the PPU produced it.
    pub rgba: Vec<u8>,
    /// Emulated frame number this was taken at, so a cover can be cited.
    pub frame: u32,
    /// How this frame scored. Higher is more title-screen-like. See
    /// [`Picker`]: it is scene duration times [`detail`], not a property of the
    /// frame alone.
    pub score: u32,
    /// Drawn detail of this frame alone, 0..=1000. See [`detail`].
    pub detail: u32,
    /// The frame the *scene* this came from began on.
    pub scene_start: u32,
    /// The machine as it stood at `scene_start`, as save-state bytes — when the
    /// caller asked for it ([`Options::keep_states`]).
    ///
    /// This is what makes the picture and the sound agree. Pokémon Gold plays a
    /// minute of intro before its title screen; without this the shelf would
    /// show the title and play the intro, and a "sixty second" export would be
    /// sixty seconds of the wrong music. With it, both start where the cover
    /// does.
    pub state: Option<Vec<u8>>,
}

impl Cover {
    /// Seconds into the boot this frame was taken.
    pub fn at_seconds(&self) -> f64 {
        self.frame as f64 / super::capture::FPS
    }
}

/// How the candidate set is gathered.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Sample a frame for scoring every this many emulated frames. Every frame
    /// would be 1.4 MB of copying per second of boot for no extra information —
    /// a title screen does not appear and vanish inside a tenth of a second.
    pub sample_every: u32,
    /// Ignore everything before this frame. Nothing interesting is on screen in
    /// the first moments; on real hardware the boot ROM is still running.
    pub warmup: u32,
    /// How many candidates to keep.
    pub keep: usize,
    /// Keep a save state at each scene's first frame, so playback and export
    /// can start where the cover does. Costs about 100 KiB per scene while the
    /// probe runs; all but the chosen ones are dropped at the end.
    pub keep_states: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            // ~6 Hz. Fine enough to catch a title screen that is only up
            // briefly, coarse enough that a 30 s probe copies ~180 frames.
            sample_every: 10,
            warmup: 8,
            keep: 4,
            keep_states: true,
        }
    }
}

/// Watches a boot go by and picks the frames most likely to be a title screen.
///
/// # Why scenes, not frames
///
/// Scoring each frame on its own picks whatever *looks* busiest, and on a real
/// cartridge that is a fade, a flash or a dithered sky in the middle of the
/// intro — measured, on Pokémon Blue and Gold, before this was rewritten.
///
/// The property that actually distinguishes a title screen is that it **stays
/// there**: the game has finished introducing itself and is waiting for the
/// player. So the picker groups consecutive samples into *scenes* (a cut is a
/// large histogram move) and scores a scene by **how long it lasted times how
/// much detail its best frame holds**. An intro animation cuts constantly and
/// scores low however pretty it is; a black screen lasts a long time and holds
/// no detail, so it scores zero; a title screen is both, and wins.
///
/// The candidates offered to the shelf are the best frame of each of the top
/// scenes, which is also what makes cycling covers useful: they are genuinely
/// different moments of the boot rather than four frames of the same fade.
pub struct Picker {
    opts: Options,
    scenes: Vec<Scene>,
    /// Whether the last sample was blank, which ends a scene.
    broke: bool,
}

struct Scene {
    /// Samples in this scene — its duration, in sample periods.
    len: u32,
    /// The most detailed frame seen in it.
    best: Cover,
    /// The frame this scene opened on.
    first_frame: u32,
    /// The machine as it stood then, if states are being kept.
    state: Option<Vec<u8>>,
    /// Histogram of the frame that *opened* the scene.
    ///
    /// **A cut is measured against this, not against the previous sample**, and
    /// that is the whole of why the picker works on a real cartridge. A slow
    /// pan or a long fade moves the picture by a fraction of a per cent between
    /// two samples a sixth of a second apart, so comparing neighbours never
    /// sees a cut and swallows an entire thirty-second intro into one "scene" —
    /// measured on Pokémon Gold, which then offered the sea behind its intro as
    /// its cover. Against a fixed anchor the drift accumulates and the scene
    /// ends where it visibly ends.
    anchor: [u64; 16],
}

impl Scene {
    /// Duration times detail. Both terms are needed: a long blank scene scores
    /// zero, and a detailed one-frame flash barely registers.
    fn score(&self, cap: u32) -> u32 {
        self.len.min(cap).saturating_mul(self.best.detail)
    }
}

/// A scene longer than this is no more title-screen-like for being longer, and
/// letting duration run away would rank a game that sits on its title for
/// twenty seconds above one that sits on a *better* title for six.
const SCENE_LEN_CAP: u32 = 40;

impl Picker {
    pub fn new(opts: Options) -> Picker {
        Picker {
            opts,
            scenes: Vec::new(),
            broke: true,
        }
    }

    /// Offer one frame. Cheap for the frames it does not want, and it keeps at
    /// most one frame per scene rather than every sample — a 30-second boot at
    /// six samples a second would otherwise hold 14 MB of pictures.
    pub fn observe(&mut self, view: &super::capture::FrameView<'_>) {
        let frame = view.number;
        // The state is only taken when a scene actually opens, so a boot pays
        // for one save state per cut rather than one per sample.
        let keep = self.opts.keep_states;
        self.observe_frame(frame, view.rgba(), &mut || keep.then(|| view.save_state()));
    }

    /// [`Picker::observe`] over a bare framebuffer, with the save state supplied
    /// lazily by a closure that is called only when a scene opens.
    ///
    /// Split out so the scene model can be tested without an emulator: the
    /// interesting behaviour is which frames become scenes, and that is pure.
    pub fn observe_frame(
        &mut self,
        frame: u32,
        rgba: &[u8],
        state: &mut dyn FnMut() -> Option<Vec<u8>>,
    ) {
        if frame < self.opts.warmup || frame % self.opts.sample_every != 0 {
            return;
        }
        if rgba.len() != WIDTH * HEIGHT * 4 {
            return;
        }
        if is_blank(rgba) {
            // A blank screen is not a scene and it also *ends* one: a game that
            // fades to black and back has not stayed on the same picture.
            self.broke = true;
            return;
        }
        let hist = histogram(rgba);
        let detail = detail(rgba);
        let cut = self.broke
            || match self.scenes.last() {
                Some(scene) => !hist_similar(&scene.anchor, &hist),
                None => true,
            };
        self.broke = false;
        let cover = Cover {
            rgba: rgba.to_vec(),
            frame,
            score: detail,
            detail,
            scene_start: frame,
            state: None,
        };
        if cut {
            self.scenes.push(Scene {
                len: 1,
                best: cover,
                first_frame: frame,
                state: state(),
                anchor: hist,
            });
            return;
        }
        let scene = self.scenes.last_mut().expect("a cut would have pushed one");
        scene.len += 1;
        if detail > scene.best.detail {
            scene.best = cover;
        }
    }

    /// The chosen candidates, best first.
    pub fn finish(self) -> Vec<Cover> {
        let Picker {
            opts, mut scenes, ..
        } = self;
        scenes.retain(|s| s.best.detail > 0);
        // Stable on ties by taking the earlier scene, so a cover is
        // reproducible run to run.
        scenes.sort_by(|a, b| {
            b.score(SCENE_LEN_CAP)
                .cmp(&a.score(SCENE_LEN_CAP))
                .then(a.best.frame.cmp(&b.best.frame))
        });
        let mut chosen: Vec<Cover> = Vec::new();
        for scene in scenes {
            if chosen.iter().any(|k| similar(&k.rgba, &scene.best.rgba)) {
                continue;
            }
            let score = scene
                .len
                .min(SCENE_LEN_CAP)
                .saturating_mul(scene.best.detail);
            let mut c = scene.best;
            c.score = score;
            c.scene_start = scene.first_frame;
            c.state = scene.state;
            chosen.push(c);
            if chosen.len() >= opts.keep {
                break;
            }
        }
        chosen
    }
}

fn hist_similar(a: &[u64; 16], b: &[u64; 16]) -> bool {
    let total: u64 = a.iter().sum();
    if total == 0 {
        return true;
    }
    let diff: u64 = a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y)).sum();
    (diff * 100) / (2 * total) < 8
}

/// How much *drawn detail* a frame holds, 0..=1000.
///
/// Counted as **distinct 8x8 blocks**, which is the unit a Game Boy actually
/// draws in. That choice does real work:
///
/// * A flat or nearly-flat screen has one or two distinct blocks: near zero.
/// * A dithered gradient — a sky, water, a fade — looks busy pixel by pixel but
///   is a handful of tiles repeated, so an edge-density measure rates it as
///   highly detailed and this one does not. That is the difference between
///   picking a game's title screen and picking the sea behind its intro.
/// * A title screen is drawn from a logo, a border and lettering: a large number
///   of blocks that occur once each.
pub fn detail(rgba: &[u8]) -> u32 {
    if rgba.len() != WIDTH * HEIGHT * 4 {
        return 0;
    }
    const B: usize = 8;
    let cols = WIDTH / B;
    let rows = HEIGHT / B;
    let mut seen: Vec<u64> = Vec::with_capacity(cols * rows);
    for by in 0..rows {
        for bx in 0..cols {
            // FNV-1a over the block's luminance bins. Binning rather than raw
            // RGB so a CGB game's near-identical shades do not read as detail.
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for y in 0..B {
                for x in 0..B {
                    let i = ((by * B + y) * WIDTH + bx * B + x) * 4;
                    let lum = (rgba[i] as u32 * 77
                        + rgba[i + 1] as u32 * 151
                        + rgba[i + 2] as u32 * 28)
                        >> 8;
                    h ^= (lum >> 4) as u64;
                    h = h.wrapping_mul(0x0000_0100_0000_01b3);
                }
            }
            seen.push(h);
        }
    }
    seen.sort_unstable();
    seen.dedup();
    // One distinct block is a flat screen and must score *zero*, not "one
    // block's worth" — so the count is of blocks beyond the first.
    let distinct = (seen.len() as u64).saturating_sub(1);
    let total = (cols * rows) as u64;
    ((distinct * 1000) / total.max(1)) as u32
}

/// Whether a frame is one flat colour — never a cover, whatever else it scores.
pub fn is_blank(rgba: &[u8]) -> bool {
    if rgba.len() != WIDTH * HEIGHT * 4 {
        return true;
    }
    histogram(rgba).iter().filter(|&&n| n > 0).count() < 2
}

/// A frame's standalone score, kept for diagnostics and for the single-frame
/// case. [`Picker`] uses [`detail`] and scene length instead — see its docs for
/// why duration is the stronger signal.
pub fn score(rgba: &[u8]) -> u32 {
    if is_blank(rgba) {
        return 0;
    }
    detail(rgba)
}

/// Whether two frames are close enough to be the same scene.
///
/// Compares 16-bin luminance histograms rather than pixels: a title screen with
/// a blinking "PRESS START" or one animated sprite differs in a few hundred
/// pixels and is the *same* screen, while a cut to another scene moves the
/// histogram substantially.
pub fn similar(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let ha = histogram(a);
    let hb = histogram(b);
    let total: u64 = ha.iter().sum();
    if total == 0 {
        return true;
    }
    let diff: u64 = ha.iter().zip(hb.iter()).map(|(x, y)| x.abs_diff(*y)).sum();
    // Half the summed absolute difference is the fraction of pixels that moved
    // bin. Under 8 % is the same screen with something small moving on it.
    (diff * 100) / (2 * total) < 8
}

fn histogram(rgba: &[u8]) -> [u64; 16] {
    let mut bins = [0u64; 16];
    for px in rgba.chunks_exact(4) {
        let lum = (px[0] as u32 * 77 + px[1] as u32 * 151 + px[2] as u32 * 28) >> 8;
        bins[(lum >> 4) as usize & 0xF] += 1;
    }
    bins
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(shade: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity(WIDTH * HEIGHT * 4);
        for _ in 0..WIDTH * HEIGHT {
            v.extend_from_slice(&[shade, shade, shade, 255]);
        }
        v
    }

    /// A busy but *repeating* picture — a dither, a sky, water. Pixel by pixel
    /// it looks like the most detailed thing on screen; as tiles it is two
    /// blocks over and over, which is exactly what it is on hardware.
    fn dithered() -> Vec<u8> {
        let mut v = Vec::with_capacity(WIDTH * HEIGHT * 4);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let s: u8 = if (x + y) % 2 == 0 { 0x20 } else { 0xE0 };
                v.extend_from_slice(&[s, s, s, 255]);
            }
        }
        v
    }

    /// A drawn screen: every 8x8 block different, the way a logo, a border and
    /// lettering are. `seed` makes two distinguishable ones.
    fn drawn(seed: u32) -> Vec<u8> {
        let mut v = Vec::with_capacity(WIDTH * HEIGHT * 4);
        let mut r: u32 = seed.wrapping_mul(2_654_435_761).max(1);
        // Different seeds also sit in different parts of the luminance range,
        // so two `drawn` screens are a *cut*, not one scene — which is what a
        // scene test needs them to be.
        let base = (seed as u8 % 4) * 0x30;
        for _ in 0..WIDTH * HEIGHT {
            r ^= r << 13;
            r ^= r >> 17;
            r ^= r << 5;
            let s = base.saturating_add((r & 0x30) as u8);
            v.extend_from_slice(&[s, s, s, 255]);
        }
        v
    }

    /// A picture with `ink_pct` per cent of its rows dark — a fade, a wipe, a
    /// letterbox. Two shades, and only three distinct tiles.
    fn banded(ink_pct: usize) -> Vec<u8> {
        let n = WIDTH * HEIGHT;
        let ink = n * ink_pct / 100;
        let mut v = Vec::with_capacity(n * 4);
        for i in 0..n {
            let s: u8 = if i < ink { 0x20 } else { 0xE0 };
            v.extend_from_slice(&[s, s, s, 255]);
        }
        v
    }

    #[test]
    fn a_blank_screen_is_never_a_cover() {
        for shade in [0x00u8, 0x80, 0xFF] {
            assert!(is_blank(&flat(shade)), "{shade:#04x}");
            assert_eq!(score(&flat(shade)), 0, "{shade:#04x}");
            assert_eq!(detail(&flat(shade)), 0, "{shade:#04x}");
        }
    }

    /// **The measurement that made the picker work.** Scoring a frame by how
    /// busy it looks picks a dithered sky in the middle of an intro; counting
    /// distinct *tiles* does not, because that sky is two blocks repeated.
    /// Measured against real cartridges before this was rewritten: Pokémon
    /// Blue's cover came out as a fade and Gold's as the sea.
    #[test]
    fn a_repeating_pattern_is_not_detail() {
        let d = detail(&dithered());
        assert!(d < 100, "a dither is a couple of tiles, not artwork: {d}");
        assert!(
            detail(&drawn(1)) > 500,
            "a drawn screen is many distinct tiles: {}",
            detail(&drawn(1))
        );
        assert!(detail(&drawn(1)) > d * 5);
        // A fade / letterbox is three tiles: near zero, whatever the split.
        for pct in [2usize, 35, 50, 97] {
            assert!(detail(&banded(pct)) < 100, "banded {pct}%");
        }
    }

    #[test]
    fn a_wrong_length_buffer_scores_zero_rather_than_panicking() {
        assert_eq!(score(&[0u8; 7]), 0);
        assert_eq!(detail(&[0u8; 7]), 0);
        assert!(is_blank(&[0u8; 7]));
        assert!(!similar(&[0u8; 8], &[0u8; 12]));
    }

    #[test]
    fn near_identical_frames_are_the_same_scene() {
        let a = drawn(1);
        let mut b = a.clone();
        // Blink something small: 1 % of the screen.
        for px in b.chunks_exact_mut(4).take(WIDTH * HEIGHT / 100) {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
        assert!(similar(&a, &b), "a blinking prompt is the same screen");
        assert!(!similar(&banded(10), &banded(60)), "a wipe is a cut");
    }

    /// **The property a title screen actually has: it stays there.** A short,
    /// detailed scene must lose to a long, detailed one.
    #[test]
    fn a_scene_that_lasts_beats_one_that_flashes_past() {
        let mut p = Picker::new(Options {
            sample_every: 1,
            warmup: 0,
            keep: 4,
            keep_states: false,
        });
        let flash = drawn(1);
        let title = drawn(2);
        for f in 1..=2 {
            p.observe_frame(f, &flash, &mut || None); // two samples: an intro beat
        }
        for f in 3..=20 {
            p.observe_frame(f, &title, &mut || None); // eighteen: the game is waiting for you
        }
        let out = p.finish();
        assert!(out.len() >= 2);
        assert_eq!(out[0].frame, 3, "the scene that lasted leads");
        assert!(
            out.iter().any(|c| c.frame <= 2),
            "the flash is still offered"
        );
    }

    /// ... and a long scene with nothing in it must lose to a short one with
    /// something. Duration alone is a black screen.
    #[test]
    fn a_long_scene_with_no_detail_scores_nothing() {
        let mut p = Picker::new(Options {
            sample_every: 1,
            warmup: 0,
            keep: 4,
            keep_states: false,
        });
        for f in 1..=40 {
            p.observe_frame(f, &flat(0x00), &mut || None); // forty samples of black
        }
        assert!(
            p.finish().is_empty(),
            "black is never a cover, however long"
        );

        let mut p = Picker::new(Options {
            sample_every: 1,
            warmup: 0,
            keep: 4,
            keep_states: false,
        });
        for f in 1..=40 {
            p.observe_frame(f, &dithered(), &mut || None);
        }
        for f in 41..=44 {
            p.observe_frame(f, &drawn(7), &mut || None);
        }
        let out = p.finish();
        assert_eq!(out[0].frame, 41, "four samples of art beat forty of dither");
    }

    #[test]
    fn candidates_are_different_moments_not_four_frames_of_one() {
        let mut p = Picker::new(Options {
            sample_every: 1,
            warmup: 0,
            keep: 3,
            keep_states: false,
        });
        // One scene, twenty samples, all the same picture.
        for f in 1..=20 {
            p.observe_frame(f, &drawn(1), &mut || None);
        }
        assert_eq!(p.finish().len(), 1, "one scene offers one cover");
    }

    #[test]
    fn the_picker_ignores_frames_before_the_warmup() {
        let mut p = Picker::new(Options {
            sample_every: 1,
            warmup: 10,
            keep: 4,
            keep_states: false,
        });
        p.observe_frame(1, &drawn(1), &mut || None);
        p.observe_frame(9, &drawn(1), &mut || None);
        assert!(p.finish().is_empty());
    }

    #[test]
    fn a_cover_can_say_when_it_was_taken() {
        let c = Cover {
            frame: 60,
            ..Cover::default()
        };
        assert!((c.at_seconds() - 1.0).abs() < 0.01, "{}", c.at_seconds());
    }
}
