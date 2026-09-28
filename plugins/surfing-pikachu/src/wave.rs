//! The wave: 29 patterns, a beach, and the state machine that picks them.
//!
//! The course is built two tiles at a time. Every 16 pixels of scroll the game
//! asks the current *wave function* for the next slice — eight metatiles
//! stacked from sky down to open water, plus the surface height at the slice's
//! left and right edge — and the function usually advances to the next one, so
//! a sequence of functions spells out one whole wave. Eight such sequences
//! exist; `SurfingMinigame_ChooseNextWaveSequence` picks between them, and at
//! section 22 it stops picking and forces the run's final big wave.
//!
//! Ported from `SurfingMinigame_GetWaveDataPointers` and its jump table in
//! pret/pokeyellow `engine/pikachu/surfing_pikachu.asm`.

use std::cmp::Ordering;

/// Open-water surface height, in the OAM Y coordinates the original uses
/// throughout (`SURFING_MINIGAME_FLAT_WATER_Y`). Screen Y is 16 less.
pub const FLAT_WATER_Y: u8 = 0x74;
const T: u8 = 8; // TILE_HEIGHT

/// Tiles that gameplay — not just the renderer — asks about by name.
pub mod tile {
    /// A slope rising to the right. Landing on one demands a nose-down board.
    pub const RISING_SLOPE: u8 = 0x06;
    /// A slope falling away to the right.
    pub const FALLING_SLOPE: u8 = 0x07;
    /// The lip of a wave. The only tile a jump can start from.
    pub const WAVE_CREST: u8 = 0x14;
    /// The steep front of a wave.
    pub const WAVE_FACE: u8 = 0x12;
}

/// One slice of course: eight metatiles top to bottom, and the water surface
/// height at its left and right edge.
pub struct Pattern {
    pub metatiles: [u8; 8],
    pub left: u8,
    pub right: u8,
}

const fn p(metatiles: [u8; 8], left: u8, right: u8) -> Pattern {
    Pattern {
        metatiles,
        left,
        right,
    }
}

/// Index into [`PATTERNS`] for the beach, which the original keeps as a
/// separate label rather than a numbered pattern.
pub const BEACH: usize = 0x1d;

/// `SurfingMinigameWavePattern00`..`1C`, then `SurfingMinigameBeachPattern`.
/// The height pairs come from each pattern's `Load...AndAdvance` routine;
/// every table entry that names a pattern uses the same one, which is why
/// they can live here rather than in the function table.
#[rustfmt::skip]
pub static PATTERNS: [Pattern; 30] = [
    p([0x00,0x00,0x00,0x01,0x01,0x01,0x01,0x01], FLAT_WATER_Y,       FLAT_WATER_Y),       // 00
    p([0x00,0x00,0x00,0x01,0x01,0x02,0x04,0x06], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 01
    p([0x00,0x00,0x00,0x01,0x02,0x04,0x06,0x0e], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - 3*T), // 02
    p([0x00,0x00,0x00,0x10,0x11,0x06,0x0e,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 5*T), // 03
    p([0x00,0x00,0x00,0x15,0x15,0x0e,0x0e,0x0e], FLAT_WATER_Y - 6*T, FLAT_WATER_Y - 6*T), // 04
    p([0x00,0x00,0x00,0x03,0x05,0x07,0x0e,0x0e], FLAT_WATER_Y - 6*T, FLAT_WATER_Y - 5*T), // 05
    p([0x00,0x00,0x00,0x01,0x03,0x05,0x07,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 3*T), // 06
    p([0x00,0x00,0x00,0x01,0x01,0x03,0x05,0x07], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - T),   // 07
    p([0x00,0x00,0x00,0x01,0x01,0x02,0x04,0x06], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 08
    p([0x00,0x00,0x00,0x01,0x02,0x04,0x06,0x0e], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - 3*T), // 09
    p([0x00,0x00,0x00,0x08,0x0f,0x0a,0x0e,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 5*T), // 0a
    p([0x00,0x00,0x00,0x09,0x0d,0x0b,0x0e,0x0e], FLAT_WATER_Y - 5*T, FLAT_WATER_Y - 5*T), // 0b
    p([0x00,0x00,0x00,0x01,0x03,0x05,0x07,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 3*T), // 0c
    p([0x00,0x00,0x00,0x01,0x01,0x03,0x05,0x07], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - T),   // 0d
    p([0x00,0x00,0x00,0x01,0x01,0x02,0x04,0x06], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 0e
    p([0x00,0x00,0x00,0x01,0x10,0x11,0x06,0x0e], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - 3*T), // 0f
    p([0x00,0x00,0x00,0x01,0x15,0x15,0x0e,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 4*T), // 10
    p([0x00,0x00,0x00,0x01,0x03,0x05,0x07,0x0e], FLAT_WATER_Y - 4*T, FLAT_WATER_Y - 3*T), // 11
    p([0x00,0x00,0x00,0x01,0x01,0x03,0x05,0x07], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - T),   // 12
    p([0x00,0x00,0x00,0x01,0x01,0x02,0x04,0x06], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 13
    p([0x00,0x00,0x00,0x01,0x08,0x0f,0x0a,0x0e], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - 3*T), // 14
    p([0x00,0x00,0x00,0x01,0x09,0x0d,0x0b,0x0e], FLAT_WATER_Y - 3*T, FLAT_WATER_Y - 3*T), // 15
    p([0x00,0x00,0x00,0x01,0x01,0x03,0x05,0x07], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - T),   // 16
    p([0x00,0x00,0x00,0x01,0x01,0x10,0x11,0x06], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 17
    p([0x00,0x00,0x00,0x01,0x01,0x15,0x15,0x0e], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - 2*T), // 18
    p([0x00,0x00,0x00,0x01,0x01,0x03,0x05,0x07], FLAT_WATER_Y - 2*T, FLAT_WATER_Y - T),   // 19
    p([0x00,0x00,0x00,0x01,0x01,0x08,0x0f,0x0a], FLAT_WATER_Y,       FLAT_WATER_Y - T),   // 1a
    p([0x00,0x00,0x00,0x01,0x01,0x09,0x0d,0x0b], FLAT_WATER_Y - T,   FLAT_WATER_Y - T),   // 1b
    p([0x00,0x00,0x00,0x14,0x14,0x14,0x14,0x14], FLAT_WATER_Y,       FLAT_WATER_Y),       // 1c
    p([0x00,0x00,0x00,0x12,0x13,0x13,0x13,0x13], FLAT_WATER_Y,       FLAT_WATER_Y),       // beach
];

/// `SurfingMinigame_BGMetatileTable` — each metatile is 2x2 tiles, stored
/// top-left, top-right, bottom-left, bottom-right.
#[rustfmt::skip]
pub static METATILES: [[u8; 4]; 0x16] = [
    [0x00,0x00,0x00,0x00], // 00 sky
    [0x0b,0x0b,0x0b,0x0b], // 01 open water
    [0x0b,0x02,0x02,0x06], // 02
    [0x03,0x0b,0x07,0x03], // 03
    [0x06,0x06,0x06,0x06], // 04
    [0x07,0x07,0x07,0x07], // 05
    [0x06,0x04,0x04,0x08], // 06
    [0x05,0x07,0x08,0x05], // 07
    [0x0b,0x0b,0x11,0x12], // 08
    [0x0b,0x0b,0x13,0x03], // 09
    [0x14,0x12,0x04,0x08], // 0a
    [0x13,0x07,0x08,0x05], // 0b
    [0x06,0x14,0x06,0x14], // 0c  unused; identical to 11
    [0x13,0x07,0x13,0x07], // 0d
    [0x08,0x08,0x08,0x08], // 0e  solid water
    [0x14,0x12,0x14,0x12], // 0f
    [0x0b,0x11,0x02,0x14], // 10
    [0x06,0x14,0x06,0x14], // 11
    [0x0c,0x0c,0x0d,0x0d], // 12  beach top
    [0x0d,0x0d,0x0d,0x0d], // 13  beach sand
    [0x0e,0x0f,0x10,0x0b], // 14  shoreline
    [0x12,0x13,0x12,0x13], // 15
];

/// What a wave function does to the function number after producing a slice.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// `SurfingMinigame_AdvanceWaveFunction` — on to the next slice.
    Advance,
    /// `SurfingMinigame_ResetWaveSequence` — back to the chooser.
    Reset,
    /// `SurfingMinigame_LoadFlatWave` — neither. The tail of the final wave
    /// parks here and produces open water for as long as the run lasts.
    Stay,
}

use Step::{Advance, Reset, Stay};

/// Entry 0 is the chooser and is handled separately; the rest is the jump
/// table from `SurfingMinigame_GetWaveDataPointers`, one `(pattern, step)`
/// per entry.
#[rustfmt::skip]
pub static WAVE_FUNCTIONS: [(usize, Step); 0x7c] = [
    (0x00, Advance), // 00: the chooser; this row is never read
    (0x13, Advance), (0x14, Advance), (0x15, Advance), (0x16, Advance),
    (0x00, Advance), (0x17, Advance), (0x18, Advance), (0x19, Advance),
    (0x00, Advance), (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 0d
    (0x08, Advance), (0x09, Advance), (0x0a, Advance), (0x0b, Advance),
    (0x0c, Advance), (0x0d, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 19
    (0x0e, Advance), (0x0f, Advance), (0x10, Advance), (0x11, Advance),
    (0x12, Advance), (0x0e, Advance), (0x0f, Advance), (0x10, Advance),
    (0x11, Advance), (0x12, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 28
    (0x13, Advance), (0x14, Advance), (0x15, Advance), (0x16, Advance),
    (0x00, Advance), (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 31
    (0x17, Advance), (0x18, Advance), (0x19, Advance), (0x17, Advance),
    (0x18, Advance), (0x19, Advance), (0x17, Advance), (0x18, Advance),
    (0x19, Advance), (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Advance),
    (0x00, Reset),   // 3f
    (0x1a, Advance), (0x1b, Advance), (0x0e, Advance), (0x0f, Advance),
    (0x10, Advance), (0x11, Advance), (0x12, Advance), (0x1a, Advance),
    (0x1b, Advance), (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 4c
    (0x08, Advance), (0x09, Advance), (0x0a, Advance), (0x0b, Advance),
    (0x0c, Advance), (0x0d, Advance), (0x00, Advance), (0x1a, Advance),
    (0x1b, Advance), (0x1a, Advance), (0x1b, Advance), (0x00, Advance),
    (0x00, Advance), (0x00, Advance),
    (0x00, Reset),   // 5b
    (0x0e, Advance), (0x0f, Advance), (0x10, Advance), (0x11, Advance),
    (0x12, Advance), (0x13, Advance), (0x14, Advance), (0x15, Advance),
    (0x16, Advance), (0x00, Advance), (0x00, Advance), (0x00, Advance),
    (0x00, Advance),
    (0x00, Reset),   // 69
    // 6a..70: the Big Kahuna, forced at section 22. 71 parks on flat water.
    (0x01, Advance), (0x02, Advance), (0x03, Advance), (0x04, Advance),
    (0x05, Advance), (0x06, Advance), (0x07, Advance),
    (0x00, Stay),    // 71
    // 72..7b: the run-out to the beach, entered when the goal is reached.
    (0x00, Advance), (0x1c, Advance),
    (BEACH, Advance), (BEACH, Advance), (BEACH, Advance), (BEACH, Advance),
    (BEACH, Advance), (BEACH, Advance), (BEACH, Advance),
    (BEACH, Reset),  // 7b
];

/// `SurfingMinigame_WaveSequenceStarts` — the eight function numbers the
/// chooser can jump to.
pub static SEQUENCE_STARTS: [u8; 8] = [0x01, 0x0e, 0x1a, 0x29, 0x32, 0x40, 0x4d, 0x5c];

/// Function number of the run's final wave, forced at section 22.
pub const BIG_KAHUNA: u8 = 0x6a;
/// Function number the goal run-out starts from.
pub const RUN_OUT: u8 = 0x72;

/// One slice of course, as [`next_slice`] hands it back.
pub struct Slice {
    pub metatiles: [u8; 8],
    pub left: u8,
    pub right: u8,
}

/// Produce the next slice and update `wave_fn`.
///
/// This is `SurfingMinigame_GetWaveDataPointers` including its entry 0. The
/// chooser is the interesting half: below section 22 it consults the random
/// byte, at exactly section 22 it forces the final wave, and *above* section
/// 22 it falls through leaving the function number alone — which, since it
/// only ever arrives here with the number at 0, means it keeps producing flat
/// water. Either way the chooser itself yields a flat slice and the sequence
/// it selected starts on the following call.
pub fn next_slice(wave_fn: &mut u8, distance_section: u8, random: u8) -> Slice {
    let idx = *wave_fn as usize;
    if idx == 0 {
        // The original's three-way branch: `jr c` below section 22, `jr z` at
        // it, `jr nc` past it — and past it the function number is left
        // alone, which since it is zero means open water to the horizon.
        match distance_section.cmp(&0x16) {
            Ordering::Less => {
                if random != 0 {
                    *wave_fn = SEQUENCE_STARTS[((random - 1) & 7) as usize];
                }
            }
            Ordering::Equal => *wave_fn = BIG_KAHUNA,
            Ordering::Greater => {}
        }
        let flat = &PATTERNS[0];
        return Slice {
            metatiles: flat.metatiles,
            left: flat.left,
            right: flat.right,
        };
    }
    let (pattern, step) = WAVE_FUNCTIONS[idx.min(WAVE_FUNCTIONS.len() - 1)];
    match step {
        Advance => *wave_fn = wave_fn.wrapping_add(1),
        Reset => *wave_fn = 0,
        Stay => {}
    }
    let pat = &PATTERNS[pattern];
    Slice {
        metatiles: pat.metatiles,
        left: pat.left,
        right: pat.right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_function_entry_names_a_real_pattern_and_metatile() {
        for (i, (pattern, _)) in WAVE_FUNCTIONS.iter().enumerate() {
            assert!(*pattern < PATTERNS.len(), "entry {i:#04x}");
        }
        for (i, pat) in PATTERNS.iter().enumerate() {
            for m in pat.metatiles {
                assert!(
                    (m as usize) < METATILES.len(),
                    "pattern {i:#04x} metatile {m:#04x}"
                );
            }
        }
    }

    #[test]
    fn every_sequence_start_terminates_in_a_reset() {
        // A sequence that never reset would strand the chooser and the course
        // would repeat one wave to the horizon.
        for start in SEQUENCE_STARTS {
            let mut f = start;
            let mut steps = 0;
            loop {
                let before = f;
                next_slice(&mut f, 0, 0);
                steps += 1;
                assert!(steps < 64, "sequence from {start:#04x} did not reset");
                if f == 0 {
                    break;
                }
                assert_ne!(f, before, "sequence stalled at {before:#04x}");
            }
        }
    }

    #[test]
    fn the_big_kahuna_is_forced_at_section_22_and_only_there() {
        let mut f = 0;
        next_slice(&mut f, 0x15, 0); // below: random 0 means "stay flat"
        assert_eq!(f, 0);
        let mut f = 0;
        next_slice(&mut f, 0x16, 0);
        assert_eq!(f, BIG_KAHUNA);
        let mut f = 0;
        next_slice(&mut f, 0x17, 0); // past it: left alone, so flat water
        assert_eq!(f, 0);
    }

    #[test]
    fn the_big_kahuna_parks_on_flat_water_rather_than_resetting() {
        let mut f = BIG_KAHUNA;
        for _ in 0..7 {
            next_slice(&mut f, 0x16, 0);
        }
        assert_eq!(f, 0x71);
        for _ in 0..10 {
            let s = next_slice(&mut f, 0x16, 0);
            assert_eq!(f, 0x71, "0x71 must not advance");
            assert_eq!(s.left, FLAT_WATER_Y);
        }
    }

    #[test]
    fn the_chooser_maps_the_random_byte_onto_the_eight_sequences() {
        // (random - 1) & 7 means 1..8 select the eight in order and 9.. wrap.
        for r in 1u8..=255 {
            let mut f = 0;
            next_slice(&mut f, 0, r);
            assert_eq!(f, SEQUENCE_STARTS[((r - 1) & 7) as usize], "random {r}");
        }
    }

    #[test]
    fn the_run_out_ends_on_beach_and_resets() {
        let mut f = RUN_OUT;
        let mut beach = 0;
        for _ in 0..10 {
            let s = next_slice(&mut f, 0x18, 0);
            if s.metatiles == PATTERNS[BEACH].metatiles {
                beach += 1;
            }
        }
        assert_eq!(beach, 8, "seven beach slices then the one that resets");
        assert_eq!(f, 0);
    }
}
