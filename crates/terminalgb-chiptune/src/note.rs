//! Notes, durations and the arithmetic that turns them into what the Game Boy
//! actually stores: an 11-bit *period* value, not a frequency.

use crate::track::CLOCK_HZ;

/// MIDI note number. 60 is middle C (`C4` here), 69 is A440.
pub type Midi = i32;

/// Parse a note name — `C4`, `C#4`, `Db4`, `A-4`, `F#-1`. Returns a MIDI number
/// with `C4 = 60`, which is the convention trackers and most DAWs use.
pub fn parse_note(s: &str) -> Option<Midi> {
    let b = s.trim().as_bytes();
    if b.is_empty() {
        return None;
    }
    let step = match b[0].to_ascii_uppercase() {
        b'C' => 0,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return None,
    };
    let mut i = 1;
    let mut accidental = 0;
    while i < b.len() {
        match b[i] {
            b'#' | b's' | b'S' => accidental += 1,
            b'b' | b'B' if i == 1 => accidental -= 1,
            _ => break,
        }
        i += 1;
    }
    // An optional separating '-' before a non-negative octave (`A-4`), kept
    // distinct from a negative octave (`C-1`) by looking at what follows.
    let rest = &s.trim()[i..];
    let rest = if rest.len() > 1 && rest.starts_with('-') && !rest[1..].starts_with('-') {
        // `C-1` is octave -1 in some notations and "C, octave 1" in others.
        // Trackers write `C-1` for octave 1, so that is what we take; a genuine
        // octave -1 is spelled `C-1-` nowhere, and `C0` is low enough for a
        // machine whose lowest square note is around C2.
        &rest[1..]
    } else {
        rest
    };
    let octave: i32 = rest.parse().ok()?;
    Some((octave + 1) * 12 + step + accidental)
}

/// Concert pitch of a MIDI note, A4 = 440 Hz.
pub fn midi_hz(n: Midi) -> f64 {
    440.0 * 2f64.powf((n - 69) as f64 / 12.0)
}

/// The 11-bit value `NR13`/`NR14` (and `NR23`/`NR24`) hold for a given pitch.
///
/// Pan Docs, "Audio Registers" → NR13: the square channels' output frequency is
/// `131072 / (2048 - x)` Hz, so `x = 2048 - 131072 / f`. There is no
/// fine-tuning below that: the period is an integer, so the reachable pitches
/// get sparse high up and the tool reports the pitch it will really produce
/// rather than the one that was asked for.
pub fn square_period(hz: f64) -> Result<u16, String> {
    period_for(hz, 131_072.0, "square")
}

/// Channel 3 counts at half the square channels' rate: `65536 / (2048 - x)`.
pub fn wave_period(hz: f64) -> Result<u16, String> {
    period_for(hz, 65_536.0, "wave")
}

fn period_for(hz: f64, numerator: f64, what: &str) -> Result<u16, String> {
    if hz <= 0.0 {
        return Err(format!("{what}: frequency must be positive"));
    }
    let x = 2048.0 - numerator / hz;
    let rounded = x.round();
    if !(0.0..=2047.0).contains(&rounded) {
        let lo = numerator / 2048.0;
        return Err(format!(
            "{hz:.2} Hz is outside the {what} channel's range \
             ({lo:.2} Hz .. {:.0} Hz); the Game Boy's period register is 11 bits",
            numerator
        ));
    }
    Ok(rounded as u16)
}

/// The pitch a period value really produces — what the chip does, not what was
/// asked for.
pub fn square_hz(period: u16) -> f64 {
    131_072.0 / (2048 - period.min(2047)) as f64
}

pub fn wave_hz(period: u16) -> f64 {
    65_536.0 / (2048 - period.min(2047)) as f64
}

/// A musical duration, held as an exact rational number of whole notes so that
/// rounding happens once, at the end, in T-cycles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Duration {
    /// `num/den` whole notes — `1/4` is a crotchet.
    Notes { num: u64, den: u64 },
    /// Wall-clock milliseconds, for people who think in time not in bars.
    Millis(u64),
    /// Raw T-cycles, for people who think in hardware.
    Cycles(u64),
}

impl Duration {
    /// Parse `1/4`, `3/8`, `1/4.` (dotted), `1/8..` (double dotted), `250ms`,
    /// `2s`, `70224t`.
    pub fn parse(s: &str) -> Result<Duration, String> {
        let s = s.trim();
        if s.is_empty() {
            return Err("empty duration".into());
        }
        if let Some(n) = s.strip_suffix("ms") {
            return n
                .trim()
                .parse::<u64>()
                .map(Duration::Millis)
                .map_err(|_| format!("bad millisecond count '{n}'"));
        }
        if let Some(n) = s.strip_suffix('t') {
            return n
                .trim()
                .parse::<u64>()
                .map(Duration::Cycles)
                .map_err(|_| format!("bad cycle count '{n}'"));
        }
        if let Some(n) = s.strip_suffix('s') {
            let secs: f64 = n
                .trim()
                .parse()
                .map_err(|_| format!("bad second count '{n}'"))?;
            if !secs.is_finite() || secs < 0.0 {
                return Err(format!("bad second count '{n}'"));
            }
            return Ok(Duration::Millis((secs * 1000.0).round() as u64));
        }
        // Trailing dots multiply by 3/2 each, as in staff notation.
        let dots = s.chars().rev().take_while(|&c| c == '.').count() as u32;
        let body = &s[..s.len() - dots as usize];
        let (num, den) = match body.split_once('/') {
            Some((a, b)) => (
                a.trim()
                    .parse::<u64>()
                    .map_err(|_| format!("bad duration '{s}'"))?,
                b.trim()
                    .parse::<u64>()
                    .map_err(|_| format!("bad duration '{s}'"))?,
            ),
            None => (
                body.trim()
                    .parse::<u64>()
                    .map_err(|_| format!("bad duration '{s}'"))?,
                1,
            ),
        };
        if den == 0 {
            return Err(format!("duration '{s}' divides by zero"));
        }
        // n dots: 2 - 2^-n, i.e. multiply by (2^(n+1) - 1) / 2^n.
        let dot_num = (1u64 << (dots + 1)) - 1;
        let dot_den = 1u64 << dots;
        Ok(Duration::Notes {
            num: num * dot_num,
            den: den * dot_den,
        })
    }

    /// Length in T-cycles at a given tempo. A whole note is four beats.
    ///
    /// Integer arithmetic throughout (`u128` intermediate, round to nearest) so
    /// the same score always produces the same cycle counts on every machine —
    /// which is what makes the renderer bit-for-bit deterministic.
    pub fn cycles(self, bpm: u32) -> u64 {
        match self {
            Duration::Cycles(c) => c,
            Duration::Millis(ms) => (ms as u128 * CLOCK_HZ as u128).div_ceil(1000) as u64,
            Duration::Notes { num, den } => {
                let n = num as u128 * 4 * 60 * CLOCK_HZ as u128;
                let d = den as u128 * bpm as u128;
                ((n + d / 2) / d) as u64
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn middle_c_is_60_and_a4_is_440() {
        assert_eq!(parse_note("C4"), Some(60));
        assert_eq!(parse_note("A4"), Some(69));
        assert!((midi_hz(69) - 440.0).abs() < 1e-9);
        assert_eq!(parse_note("C#4"), Some(61));
        assert_eq!(parse_note("Db4"), Some(61));
        assert_eq!(parse_note("A-4"), Some(69));
        assert_eq!(parse_note("B3"), Some(59));
        assert_eq!(parse_note("Q4"), None);
    }

    #[test]
    fn the_period_register_matches_pan_docs() {
        // A 440 Hz tone: 2048 - 131072/440 = 1750.05 -> 1750.
        let p = square_period(440.0).unwrap();
        assert_eq!(p, 1750);
        // And the pitch it really produces is the one we report.
        assert!((square_hz(p) - 439.84).abs() < 0.01);
        // Channel 3 counts half as fast, so the same note needs a different
        // period — getting this backwards transposes the wave channel an
        // octave, which is the classic Game Boy bug.
        assert_eq!(wave_period(440.0).unwrap(), 1899);
    }

    #[test]
    fn a_note_below_the_chip_is_an_error_naming_the_range() {
        let e = square_period(midi_hz(parse_note("C1").unwrap())).unwrap_err();
        assert!(e.contains("64.00 Hz"), "{e}");
    }

    #[test]
    fn durations_are_exact_integers() {
        // At 120 bpm a crotchet is half a second exactly.
        assert_eq!(
            Duration::parse("1/4").unwrap().cycles(120),
            CLOCK_HZ / 2,
            "quarter note at 120bpm"
        );
        // A dotted crotchet is three quavers.
        assert_eq!(
            Duration::parse("1/4.").unwrap().cycles(120),
            Duration::parse("3/8").unwrap().cycles(120)
        );
        assert_eq!(Duration::parse("250ms").unwrap().cycles(120), CLOCK_HZ / 4);
        assert_eq!(Duration::parse("1234t").unwrap().cycles(120), 1234);
        assert!(Duration::parse("1/0").is_err());
    }
}
