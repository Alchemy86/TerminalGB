//! The register script: the honest hardware interface, in text.
//!
//! One write per line, `<time> <register> <value>`. This is the format that
//! cannot be wrong, because it is exactly what a cartridge puts on the bus —
//! everything else in this crate compiles to it, and [`Track::to_register_script`]
//! prints it back out, so a composer can always see what the chip is really
//! being told.
//!
//! ```text
//! model dmg
//! 0t       NR52 $80      # power on
//! 0t       NR50 $77
//! 0t       NR51 $FF
//! 0t       NR11 $80      # 50% duty
//! 0t       NR12 $F0      # volume 15, no envelope
//! 0t       NR13 $D6      # 440 Hz, low byte
//! 0t       NR14 $86      #          high bits + trigger
//! +500ms   NR12 $00      # DAC off — note ends
//! end 1s
//! ```
//!
//! Times are absolute unless prefixed with `+`, which makes them relative to
//! the previous line's time. Any duration spelling [`crate::note::Duration`]
//! understands works: `1234t`, `250ms`, `1.5s`, or a note value like `1/8`
//! once a `tempo` has been given.
//!
//! [`Track::to_register_script`]: crate::Track::to_register_script

use crate::note::Duration;
use crate::track::{reg_addr, Model, Track};

pub fn compile(src: &str) -> Result<Track, String> {
    let mut track = Track::new();
    let mut bpm: u32 = 120;
    let mut prev: u64 = 0;
    let mut end: Option<u64> = None;

    for (lineno, raw) in src.lines().enumerate() {
        let line = match raw.find(['#', ';']) {
            Some(i) => &raw[..i],
            None => raw,
        }
        .trim();
        if line.is_empty() {
            continue;
        }
        let err = |e: String| format!("line {}: {e}", lineno + 1);
        let tok: Vec<&str> = line.split_whitespace().collect();

        match tok[0].to_ascii_lowercase().as_str() {
            "model" => {
                let m = tok
                    .get(1)
                    .ok_or_else(|| err("model needs dmg or cgb".into()))?;
                track.model = Model::parse(m)
                    .ok_or_else(|| err(format!("unknown model '{m}' — dmg or cgb")))?;
                continue;
            }
            "tempo" => {
                bpm = tok
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .filter(|&b| b > 0)
                    .ok_or_else(|| err("tempo needs a positive number".into()))?;
                continue;
            }
            "title" => {
                track.title = Some(tok[1..].join(" "));
                continue;
            }
            "end" => {
                let d = tok
                    .get(1)
                    .ok_or_else(|| err("end needs a duration".into()))?;
                end = Some(time_of(d, prev, bpm).map_err(err)?);
                continue;
            }
            _ => {}
        }

        if tok.len() != 3 {
            return Err(err(format!(
                "expected '<time> <register> <value>', got {} field(s)",
                tok.len()
            )));
        }
        let at = time_of(tok[0], prev, bpm).map_err(err)?;
        let reg = reg_addr(tok[1])
            .ok_or_else(|| err(format!("unknown register '{}'", tok[1])))?;
        let value = parse_byte(tok[2]).map_err(err)?;
        track.push(at, reg, value);
        prev = at;
    }

    track.sort();
    let last = track.writes.last().map(|w| w.at).unwrap_or(0);
    track.duration = end.unwrap_or(last);
    track.validate()?;
    Ok(track)
}

/// `+<duration>` is relative to the previous line; anything else is absolute.
fn time_of(s: &str, prev: u64, bpm: u32) -> Result<u64, String> {
    match s.strip_prefix('+') {
        Some(rest) => Ok(prev + Duration::parse(rest)?.cycles(bpm)),
        None => Ok(Duration::parse(s)?.cycles(bpm)),
    }
}

fn parse_byte(s: &str) -> Result<u8, String> {
    let t = s.trim();
    if let Some(h) = t.strip_prefix('$').or_else(|| t.strip_prefix("0x")) {
        return u8::from_str_radix(h, 16).map_err(|_| format!("bad byte '{s}'"));
    }
    if let Some(b) = t.strip_prefix('%') {
        return u8::from_str_radix(b, 2).map_err(|_| format!("bad byte '{s}'"));
    }
    t.parse().map_err(|_| format!("bad byte '{s}'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_times_accumulate() {
        let t = compile(
            "0t   NR52 $80\n\
             +10t NR50 $77\n\
             +10t NR51 $FF\n\
             end +100t\n",
        )
        .unwrap();
        assert_eq!(
            t.writes.iter().map(|w| w.at).collect::<Vec<_>>(),
            [0, 10, 20]
        );
        assert_eq!(t.duration, 120);
    }

    #[test]
    fn every_byte_spelling_parses() {
        let t = compile("0t NR52 $80\n1t NR50 128\n2t NR51 %10000000\nend 3t\n").unwrap();
        assert!(t.writes.iter().all(|w| w.value == 0x80));
    }

    #[test]
    fn an_unknown_register_names_its_line() {
        let e = compile("0t NR52 $80\n1t NRXX $00\n").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn a_musical_time_works_once_a_tempo_is_given() {
        let t = compile("tempo 120\n1/4 NR52 $80\nend 1/2\n").unwrap();
        assert_eq!(t.writes[0].at, crate::track::CLOCK_HZ / 2);
        assert_eq!(t.duration, crate::track::CLOCK_HZ);
    }
}
