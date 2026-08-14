//! The friendly layer: a tiny line-oriented score that compiles to register
//! writes.
//!
//! It is deliberately **not** a tracker language. Almost nobody thinks in
//! `NR12 = $F3`, so without something on top the register layer is a tool only
//! its author can use — but a whole music language would be a second project
//! and would put a translator between the composer and the chip, which is the
//! thing this tool exists to remove. So: one construct per hardware feature,
//! named after the hardware feature, and an escape hatch (`raw`) straight down
//! to the registers for anything not covered.
//!
//! ```text
//! tempo 120
//! model dmg
//!
//! ch1 duty 2
//! ch1 env 15 - 2
//! ch1 note C4 1/8
//! ch1 note E4 1/8
//! ch1 note G4 1/4
//!
//! ch2 vol 8
//! ch2 note C3 1/2
//!
//! ch4 noise 4 15 4
//! ch4 hit 1/16
//! ```
//!
//! **Each channel keeps its own cursor**, starting at time zero and advancing
//! by each note or rest it plays. That is what makes four-part writing fall out
//! with no bar or pattern concept: write channel 1's melody, then channel 2's
//! bass, and they line up because both started at zero.

use crate::note::{midi_hz, parse_note, square_period, wave_period, Duration};
use crate::track::{reg_addr, Model, Track, CLOCK_HZ, POWER_ON};

/// Default silence after the last note, so a render does not stop dead on the
/// final sample.
const DEFAULT_TAIL_MS: u64 = 120;

#[derive(Clone, Copy)]
struct ChannelState {
    cursor: u64,
    duty: u8,
    /// NRx2 as it will be written: volume, direction, period.
    env: u8,
    /// NR10 for channel 1.
    sweep: u8,
    /// NR32 output level for channel 3.
    level: u8,
    /// NR43 for channel 4.
    noise: u8,
}

impl ChannelState {
    fn new() -> ChannelState {
        ChannelState {
            cursor: 0,
            duty: 2,     // 50% — the neutral square
            env: 0xF0,   // volume 15, no envelope sweep
            sweep: 0x00, // off
            level: 0x20, // 100%
            noise: 0x44, // shift 4, 15-bit, divisor 4 — a plain hiss
        }
    }
}

/// Parse a score and compile it to register writes.
pub fn compile(src: &str) -> Result<Track, String> {
    let mut track = Track::new();
    let mut bpm: u32 = 120;
    let mut power = true;
    let mut tail: Option<Duration> = None;
    let mut end: Option<Duration> = None;
    let mut nr51: u8 = 0xFF;
    let mut nr50: u8 = 0x77;
    let mut ch = [ChannelState::new(); 4];
    // Deferred, because they belong at time zero *after* the power-on writes
    // and we do not know the panning until the whole score is read.
    let mut extra: Vec<(u64, u16, u8)> = Vec::new();

    for (lineno, raw) in src.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let err = |e: String| format!("line {}: {e}", lineno + 1);
        let mut tok = line.split_whitespace();
        let head = tok.next().unwrap();
        let rest: Vec<&str> = tok.collect();

        match head.to_ascii_lowercase().as_str() {
            "title" => track.title = Some(rest.join(" ")),
            "tempo" => {
                bpm = one(&rest, "tempo")
                    .map_err(err)?
                    .parse()
                    .map_err(|_| err(format!("bad tempo '{}'", rest[0])))?;
                if bpm == 0 {
                    return Err(err("tempo must be above zero".into()));
                }
            }
            "model" => {
                let m = one(&rest, "model").map_err(err)?;
                track.model = Model::parse(m)
                    .ok_or_else(|| err(format!("unknown model '{m}' — dmg or cgb")))?;
            }
            "power" => {
                power = match one(&rest, "power").map_err(err)? {
                    "on" => true,
                    "off" => false,
                    other => {
                        return Err(err(format!("power takes on or off, not '{other}'")))
                    }
                }
            }
            "master" => {
                let (l, r) = two(&rest, "master").map_err(err)?;
                let l: u8 = l.parse().map_err(|_| err("master needs two 0-7".into()))?;
                let r: u8 = r.parse().map_err(|_| err("master needs two 0-7".into()))?;
                if l > 7 || r > 7 {
                    return Err(err("master volumes are 0-7".into()));
                }
                nr50 = (l << 4) | r;
            }
            "tail" => {
                tail =
                    Some(Duration::parse(one(&rest, "tail").map_err(err)?).map_err(err)?)
            }
            "end" => {
                end = Some(Duration::parse(one(&rest, "end").map_err(err)?).map_err(err)?)
            }
            "write" => {
                // `write <time> <reg> <value>` — an absolute-time escape hatch.
                if rest.len() != 3 {
                    return Err(err("write takes <time> <register> <value>".into()));
                }
                let at = Duration::parse(rest[0]).map_err(err)?.cycles(bpm);
                let reg = reg_addr(rest[1])
                    .ok_or_else(|| err(format!("unknown register '{}'", rest[1])))?;
                let v = parse_byte(rest[2]).map_err(err)?;
                extra.push((at, reg, v));
            }
            c if c.starts_with("ch") && c.len() == 3 => {
                let idx = match c.as_bytes()[2] {
                    b'1' => 0,
                    b'2' => 1,
                    b'3' => 2,
                    b'4' => 3,
                    _ => return Err(err(format!("unknown channel '{c}' — ch1..ch4"))),
                };
                channel_line(idx, &rest, bpm, &mut ch[idx], &mut nr51, &mut track)
                    .map_err(err)?;
            }
            other => {
                return Err(err(format!(
                    "unknown directive '{other}' — expected tempo, model, title, \
                     power, master, tail, end, write or chN"
                )))
            }
        }
    }

    // Preamble first, at time zero, so nothing can land while the APU is off.
    let mut head: Vec<(u64, u16, u8)> = Vec::new();
    if power {
        for (r, v) in POWER_ON {
            // The score's own `master` and `pan` settings replace the defaults
            // in place, so the canonical order is kept and nothing is written
            // twice.
            let v = match r {
                0xFF24 => nr50,
                0xFF25 => nr51,
                _ => v,
            };
            head.push((0, r, v));
        }
    }
    head.extend(extra);
    // Prepending keeps stable-sort order: everything at cycle 0 in the score
    // still follows the power-on it depends on.
    let body = std::mem::take(&mut track.writes);
    track.writes = head
        .into_iter()
        .map(|(a, r, v)| crate::track::RegisterWrite::new(a, r, v))
        .collect();
    track.writes.extend(body);
    track.sort();

    let last = ch.iter().map(|c| c.cursor).max().unwrap_or(0);
    let tail = tail
        .map(|d| d.cycles(bpm))
        .unwrap_or(DEFAULT_TAIL_MS * CLOCK_HZ / 1000);
    track.duration = end.map(|d| d.cycles(bpm)).unwrap_or(last + tail);
    track.validate()?;
    Ok(track)
}

fn strip_comment(s: &str) -> &str {
    match s.find(['#', ';']) {
        Some(i) => &s[..i],
        None => s,
    }
}

fn one<'a>(rest: &[&'a str], what: &str) -> Result<&'a str, String> {
    rest.first()
        .copied()
        .ok_or_else(|| format!("{what} needs an argument"))
}

fn two<'a>(rest: &[&'a str], what: &str) -> Result<(&'a str, &'a str), String> {
    if rest.len() < 2 {
        return Err(format!("{what} needs two arguments"));
    }
    Ok((rest[0], rest[1]))
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

/// `+` / `up` = the envelope (or sweep) counts upward; `-` / `down` = downward;
/// `0` / `off` = it does not run at all.
fn parse_direction(s: &str) -> Result<Option<bool>, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "+" | "up" | "inc" => Ok(Some(true)),
        "-" | "down" | "dec" => Ok(Some(false)),
        "0" | "off" | "none" | "hold" | "flat" => Ok(None),
        other => Err(format!("direction must be +, - or 0 (not '{other}')")),
    }
}

/// Base address of each channel's register block: NR10, NR20(unused)/NR21,
/// NR30, NR40(unused)/NR41. Indexed so `base + n` names NRx0..NRx4.
const CH_BASE: [u16; 4] = [0xFF10, 0xFF15, 0xFF1A, 0xFF1F];

fn channel_line(
    idx: usize,
    rest: &[&str],
    bpm: u32,
    st: &mut ChannelState,
    nr51: &mut u8,
    track: &mut Track,
) -> Result<(), String> {
    let base = CH_BASE[idx];
    let Some(&cmd) = rest.first() else {
        return Err("a channel line needs a command".into());
    };
    let args = &rest[1..];
    let lower = cmd.to_ascii_lowercase();

    // `ch1 C4 1/8` is shorthand for `ch1 note C4 1/8`.
    let (lower, args) = if parse_note(cmd).is_some() && lower != "note" {
        ("note".to_string(), rest)
    } else {
        (lower, args)
    };

    match lower.as_str() {
        "duty" => {
            let d: u8 = one(args, "duty")?
                .parse()
                .map_err(|_| "duty is 0-3 (12.5%, 25%, 50%, 75%)".to_string())?;
            if d > 3 {
                return Err("duty is 0-3".into());
            }
            st.duty = d;
        }
        "vol" => {
            let v: u8 = one(args, "vol")?
                .parse()
                .map_err(|_| "vol is 0-15".to_string())?;
            if v > 15 {
                return Err("vol is 0-15".into());
            }
            st.env = v << 4;
        }
        "env" => {
            if args.len() != 3 {
                return Err("env takes <volume 0-15> <+|-|0> <period 0-7>".into());
            }
            let v: u8 = args[0]
                .parse()
                .map_err(|_| "env volume is 0-15".to_string())?;
            let dir = parse_direction(args[1])?;
            let p: u8 = args[2]
                .parse()
                .map_err(|_| "env period is 0-7".to_string())?;
            if v > 15 || p > 7 {
                return Err("env is <volume 0-15> <+|-|0> <period 0-7>".into());
            }
            // Direction "off" and period 0 both mean the same thing to the
            // chip: the counter never fires. Keep the volume.
            let up = matches!(dir, Some(true));
            let period = if dir.is_none() { 0 } else { p };
            st.env = (v << 4) | if up { 0x08 } else { 0 } | period;
        }
        "sweep" => {
            if idx != 0 {
                return Err("only channel 1 has a frequency sweep".into());
            }
            if args.len() == 1 && args[0].eq_ignore_ascii_case("off") {
                st.sweep = 0;
            } else if args.len() == 3 {
                let p: u8 = args[0]
                    .parse()
                    .map_err(|_| "sweep period is 0-7".to_string())?;
                let dir = parse_direction(args[1])?;
                let sh: u8 = args[2]
                    .parse()
                    .map_err(|_| "sweep shift is 0-7".to_string())?;
                if p > 7 || sh > 7 {
                    return Err("sweep is <period 0-7> <+|-> <shift 0-7>".into());
                }
                // NR10 bit 3 set = *decrease* the frequency. `+` in the score
                // means the pitch rises, so it clears the bit.
                let down = matches!(dir, Some(false));
                st.sweep = (p << 4) | if down { 0x08 } else { 0 } | sh;
            } else {
                return Err("sweep takes <period> <+|-> <shift>, or 'off'".into());
            }
        }
        "level" => {
            if idx != 2 {
                return Err("only channel 3 has an output level".into());
            }
            let l: u8 = one(args, "level")?
                .parse()
                .map_err(|_| "level is 0-3 (mute, 100%, 50%, 25%)".to_string())?;
            if l > 3 {
                return Err("level is 0-3".into());
            }
            st.level = l << 5;
        }
        "wave" => {
            if idx != 2 {
                return Err("only channel 3 has a wavetable".into());
            }
            let hex: String = args.concat();
            if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(format!(
                    "wave takes 32 hex digits (one 4-bit sample each), got {}",
                    hex.len()
                ));
            }
            // The DAC has to be off while wave RAM is loaded — a write that
            // lands while channel 3 is playing hits the byte the channel is
            // fetching, which on a DMG is the wave-RAM corruption quirk rather
            // than the sample you meant.
            track.push(st.cursor, 0xFF1A, 0x00);
            for i in 0..16 {
                let hi = hex.as_bytes()[i * 2] as char;
                let lo = hex.as_bytes()[i * 2 + 1] as char;
                let b = (hi.to_digit(16).unwrap() as u8) << 4
                    | lo.to_digit(16).unwrap() as u8;
                track.push(st.cursor, 0xFF30 + i as u16, b);
            }
        }
        "noise" => {
            if idx != 3 {
                return Err("only channel 4 is the noise channel".into());
            }
            if args.len() != 3 {
                return Err("noise takes <shift 0-13> <width 15|7> <divisor 0-7>".into());
            }
            let sh: u8 = args[0]
                .parse()
                .map_err(|_| "noise shift is 0-13".to_string())?;
            let width: u8 = args[1]
                .parse()
                .map_err(|_| "noise width is 15 or 7".to_string())?;
            let div: u8 = args[2]
                .parse()
                .map_err(|_| "noise divisor is 0-7".to_string())?;
            if sh > 13 || div > 7 || (width != 7 && width != 15) {
                return Err("noise is <shift 0-13> <width 15|7> <divisor 0-7>".into());
            }
            st.noise = (sh << 4) | if width == 7 { 0x08 } else { 0 } | div;
        }
        "pan" => {
            let bit = 1u8 << idx;
            let (l, r) = match one(args, "pan")?.to_ascii_lowercase().as_str() {
                "both" | "centre" | "center" | "c" => (true, true),
                "left" | "l" => (true, false),
                "right" | "r" => (false, true),
                "off" | "mute" => (false, false),
                other => {
                    return Err(format!(
                        "pan takes left, right, both or off (not '{other}')"
                    ))
                }
            };
            // NR51: high nibble = left, low nibble = right.
            *nr51 = (*nr51 & !(bit << 4) & !bit)
                | if l { bit << 4 } else { 0 }
                | if r { bit } else { 0 };
            track.push(st.cursor, 0xFF25, *nr51);
        }
        "raw" => {
            let (r, v) = two(args, "raw")?;
            let reg = reg_addr(r).ok_or_else(|| format!("unknown register '{r}'"))?;
            track.push(st.cursor, reg, parse_byte(v)?);
        }
        "at" => {
            st.cursor = Duration::parse(one(args, "at")?)?.cycles(bpm);
        }
        "rest" => {
            st.cursor += Duration::parse(one(args, "rest")?)?.cycles(bpm);
        }
        "note" | "hit" => {
            let (name, dur) = if lower == "hit" {
                (None, one(args, "hit")?)
            } else {
                let (n, d) = two(args, "note")?;
                (Some(n), d)
            };
            let len = Duration::parse(dur)?.cycles(bpm);
            let t = st.cursor;
            emit_note(idx, base, name, t, st, track)?;
            st.cursor = t + len;
            emit_note_off(idx, base, st.cursor, track);
        }
        other => {
            return Err(format!(
                "unknown channel command '{other}' — expected note, hit, rest, at, \
                 duty, vol, env, sweep, level, wave, noise, pan or raw"
            ))
        }
    }
    Ok(())
}

fn emit_note(
    idx: usize,
    base: u16,
    name: Option<&str>,
    t: u64,
    st: &ChannelState,
    track: &mut Track,
) -> Result<(), String> {
    match idx {
        0 | 1 => {
            let name = name.ok_or("a square channel needs a note name")?;
            let midi =
                parse_note(name).ok_or_else(|| format!("unknown note '{name}'"))?;
            let period = square_period(midi_hz(midi))?;
            if idx == 0 {
                track.push(t, base, st.sweep); // NR10
            }
            track.push(t, base + 1, st.duty << 6); // NRx1: duty, length load 0
            track.push(t, base + 2, st.env); // NRx2
            track.push(t, base + 3, (period & 0xFF) as u8); // NRx3
                                                            // NRx4: trigger, length counter left disabled — the note is ended
                                                            // by the note-off below, not by the 64-step counter (which tops out
                                                            // at a quarter of a second and cannot hold a long note).
            track.push(t, base + 4, 0x80 | (period >> 8) as u8);
        }
        2 => {
            let name = name.ok_or("channel 3 needs a note name")?;
            let midi =
                parse_note(name).ok_or_else(|| format!("unknown note '{name}'"))?;
            let period = wave_period(midi_hz(midi))?;
            track.push(t, 0xFF1A, 0x80); // NR30: DAC on
            track.push(t, 0xFF1C, st.level); // NR32
            track.push(t, 0xFF1D, (period & 0xFF) as u8); // NR33
            track.push(t, 0xFF1E, 0x80 | (period >> 8) as u8); // NR34
        }
        _ => {
            if name.is_some() {
                return Err("channel 4 has no pitch — use `ch4 hit <duration>`".into());
            }
            track.push(t, 0xFF20, 0x00); // NR41: length load (unused, length off)
            track.push(t, 0xFF21, st.env); // NR42
            track.push(t, 0xFF22, st.noise); // NR43
            track.push(t, 0xFF23, 0x80); // NR44: trigger
        }
    }
    Ok(())
}

/// End a note by turning the channel's DAC off, which is what a real driver
/// does: writing zero to NRx2 (or NR30 for the wave channel) clears the enable
/// flip-flop `NR52` reports, and the output falls silent on that cycle.
fn emit_note_off(idx: usize, base: u16, t: u64, track: &mut Track) {
    match idx {
        0 | 1 => track.push(t, base + 2, 0x00),
        2 => track.push(t, 0xFF1A, 0x00),
        _ => track.push(t, 0xFF21, 0x00),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn writes_of(src: &str) -> Track {
        compile(src).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_score_powers_the_chip_on_before_anything_else() {
        let t = writes_of("ch1 note C4 1/4\n");
        assert_eq!(t.writes[0].reg, 0xFF26);
        assert_eq!(t.writes[0].value, 0x80);
        assert_eq!(t.writes[0].at, 0);
    }

    #[test]
    fn each_channel_keeps_its_own_cursor_so_parts_line_up() {
        let t = writes_of(
            "tempo 120\n\
             ch1 note C4 1/4\n\
             ch1 note E4 1/4\n\
             ch2 note C3 1/2\n",
        );
        // Channel 2's single note must start at zero, not after channel 1's
        // two — that is the whole point of a per-channel cursor.
        let ch2_trigger = t
            .writes
            .iter()
            .find(|w| w.reg == 0xFF19)
            .expect("channel 2 trigger");
        assert_eq!(ch2_trigger.at, 0);
        // And channel 1's second note starts exactly one crotchet in.
        let ch1_triggers: Vec<_> = t.writes.iter().filter(|w| w.reg == 0xFF14).collect();
        assert_eq!(ch1_triggers.len(), 2);
        assert_eq!(ch1_triggers[1].at, CLOCK_HZ / 2);
    }

    #[test]
    fn a_note_is_ended_by_switching_the_dac_off() {
        let t = writes_of("tempo 120\nch1 note C4 1/4\n");
        let off = t
            .writes
            .iter()
            .find(|w| w.reg == 0xFF12 && w.value == 0)
            .expect("note off");
        assert_eq!(off.at, CLOCK_HZ / 2);
    }

    #[test]
    fn the_shorthand_and_the_long_form_compile_identically() {
        let a = writes_of("ch1 note C4 1/8\n");
        let b = writes_of("ch1 C4 1/8\n");
        assert_eq!(a.writes, b.writes);
    }

    #[test]
    fn the_wave_table_is_loaded_with_the_dac_off() {
        let t = writes_of("ch3 wave 0123456789ABCDEFFEDCBA9876543210\nch3 note C4 1/4\n");
        let first_wave = t.writes.iter().position(|w| w.reg == 0xFF30).unwrap();
        let dac_off = t
            .writes
            .iter()
            .position(|w| w.reg == 0xFF1A && w.value == 0)
            .unwrap();
        assert!(
            dac_off < first_wave,
            "DAC must be off before wave RAM lands"
        );
        assert_eq!(t.writes[first_wave].value, 0x01);
    }

    #[test]
    fn sweep_direction_matches_the_hardware_bit() {
        // `-` (pitch falls) sets NR10 bit 3; `+` clears it. Getting this
        // backwards is silent and inverts every sweep.
        let down = writes_of("ch1 sweep 3 - 2\nch1 note C4 1/4\n");
        let up = writes_of("ch1 sweep 3 + 2\nch1 note C4 1/4\n");
        let nr10 = |t: &Track| t.writes.iter().find(|w| w.reg == 0xFF10).unwrap().value;
        assert_eq!(nr10(&down), 0x3A);
        assert_eq!(nr10(&up), 0x32);
    }

    #[test]
    fn panning_only_touches_its_own_channel_bits() {
        let t = writes_of("ch1 pan left\nch2 pan right\nch1 note C4 1/8\n");
        let last = t.writes.iter().filter(|w| w.reg == 0xFF25).last().unwrap();
        // NR51 high nibble = left, low = right. Starting from $FF: ch1 keeps
        // only its left bit ($10) and ch2 only its right bit ($02); channels 3
        // and 4 keep both of theirs.
        assert_eq!(last.value, 0xDE, "got ${:02X}", last.value);
    }

    #[test]
    fn a_typo_names_the_line_it_is_on() {
        let e = compile("tempo 120\nch1 nte C4 1/4\n").unwrap_err();
        assert!(e.starts_with("line 2:"), "{e}");
    }

    #[test]
    fn a_note_the_chip_cannot_reach_is_refused() {
        let e = compile("ch1 note C0 1/4\n").unwrap_err();
        assert!(e.contains("outside"), "{e}");
    }

    #[test]
    fn a_pitch_only_command_on_the_noise_channel_is_refused() {
        assert!(compile("ch4 note C4 1/4\n").is_err());
        assert!(compile("ch1 hit 1/4\n").is_err());
        assert!(compile("ch2 sweep 1 - 1\n").is_err());
    }

    #[test]
    fn the_register_script_round_trips() {
        let t = writes_of("tempo 140\nmodel cgb\nch1 note A4 1/4\nch2 note E4 1/4\n");
        let script = t.to_register_script();
        let back = crate::regs::compile(&script).unwrap();
        assert_eq!(back.writes, t.writes);
        assert_eq!(back.duration, t.duration);
        assert_eq!(back.model, t.model);
    }
}
