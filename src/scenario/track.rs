//! A run's position track, and the compact form it is stored in.
//!
//! # Why this exists
//!
//! **Every run records where the player was, so any run can be filmed later.**
//! It used to be opt-in (`corpus --paths`), which meant a certification pass
//! and a film pass were two different runs of the same work — and the
//! consequence was that *the runs that were actually certified could never be
//! filmed*. Wanting a picture of a certified attempt meant re-running the whole
//! thing, and the interesting material — the failures, the blackouts, the tail
//! — was thrown away at the moment it happened and could not be recovered.
//!
//! The reason it was opt-in was cost, and cost is a **format** problem rather
//! than a reason to discard the data. Two decisions take it from gigabytes to
//! tens of megabytes for a 3,000-attempt certification, and both are measured
//! rather than argued:
//!
//! 1. **The walk counter is not part of a position track.** `wWalkCounter`
//!    ($CFC5) ticks 7,6,5,4,3,2,1 across a step, so a *walking* agent used to
//!    cost a row every two frames — measured over three real corpora, **83 % of
//!    all rows changed nothing but that counter**. Nothing downstream has ever
//!    read it: the renderer reconstructs the glide by interpolating between
//!    consecutive recorded tiles (`chainrender.tracks.interpolate`), which is
//!    independent of the sampling rate and works on tracks already on disk.
//!    So the counter is dropped and the step is expanded back at draw time,
//!    which is exactly what "store transitions, not frames" means here.
//! 2. **A record is a delta, in bytes.** A CSV row is ~19 bytes of ASCII; a
//!    record here is a one-byte field mask, a varint frame delta and only the
//!    bytes that changed — **3.2 bytes measured**.
//!
//! Together: **35-43x smaller**, measured against three real corpora before and
//! after (see `docs/agent/track-format.md`). A whole fourteen-link playthrough
//! is ~28 KB per agent instead of ~1.1 MB.
//!
//! # The format — `GBTK`, version 1
//!
//! ```text
//! magic   "GBTK"        4 bytes
//! version u8            1
//! flags   u8            0; reserved, refused when non-zero
//! count   u32 LE        records that follow
//! last    u32 LE        the last emulated frame the run reached
//! records...
//! ```
//!
//! Each record is a field mask, a frame delta and the changed fields:
//!
//! ```text
//! mask    u8            bit0 map, bit1 x, bit2 y, bit3 facing, bit4 battle
//! dframe  varint        frames since the previous record (LEB128)
//! bytes                 one per set bit, in bit order
//! ```
//!
//! The first record carries every field, so a decoder never has to invent a
//! starting value. Frames are absolute in the file's own terms: the first
//! delta is measured from zero.
//!
//! # What it deliberately does not carry
//!
//! Pixels. A position track says where the player was and nothing about what
//! was on screen, which is why a mosaic of many runs needs
//! [`crate::scenario::screens`] and cannot be derived from a corpus afterwards.
//! That asymmetry is the point: positions are cheap enough to keep for every
//! run, and pictures are not.

/// One recorded change of where the player was.
///
/// A point is written when any of these five move, and at no other time — so a
/// standing character costs one record however long it stands. `frame` is the
/// emulated frame the state *began* at, which is what makes the gap to the
/// next record a dwell time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrackPoint {
    pub frame: u32,
    pub map: u8,
    pub x: u8,
    pub y: u8,
    /// 0 down, 4 up, 8 left, 12 right — the game's own encoding.
    pub facing: u8,
    /// `wIsInBattle` **verbatim**: 1 wild, 2 trainer, `$FF` blacked out. Do not
    /// read it as a boolean — `$FF` is the end of a run, not a fight, and a
    /// catch-all over that byte has cost this repository four separate bugs.
    pub in_battle: u8,
}

impl TrackPoint {
    fn state(&self) -> (u8, u8, u8, u8, u8) {
        (self.map, self.x, self.y, self.facing, self.in_battle)
    }
}

/// Where the player was, for one run.
///
/// **Deduplicating happens on the way in, not on the way out.** A run of half a
/// million frames used to push half a million points into a `Vec` and collapse
/// them when the file was written; with recording on by default that is a few
/// megabytes of live memory per worker in a sixteen-worker fan-out, for data
/// that is 17 % of its own size. `push` keeps the last state and drops a point
/// that changed nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Track {
    points: Vec<TrackPoint>,
    last_frame: u32,
}

/// `GBTK`.
pub const MAGIC: [u8; 4] = *b"GBTK";
/// The version this build writes and the only one it reads.
pub const VERSION: u8 = 1;
const HEADER: usize = 14;

impl Track {
    pub fn new() -> Track {
        Track::default()
    }

    /// Record a point, keeping it only if something moved.
    pub fn push(&mut self, p: TrackPoint) {
        if p.frame > self.last_frame {
            self.last_frame = p.frame;
        }
        match self.points.last() {
            Some(l) if l.state() == p.state() => {}
            _ => self.points.push(p),
        }
    }

    /// The recorded changes, in frame order.
    pub fn points(&self) -> &[TrackPoint] {
        &self.points
    }

    /// The last emulated frame the run reached. Not the last *record*: a run
    /// that ends standing still has its final record long before its final
    /// frame, and a film needs to know how long the agent was there.
    pub fn last_frame(&self) -> u32 {
        self.last_frame
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    /// The whole track as `GBTK` bytes.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.points.len() * 4);
        out.extend_from_slice(&MAGIC);
        out.push(VERSION);
        out.push(0);
        out.extend_from_slice(&(self.points.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.last_frame.to_le_bytes());
        let mut prev: Option<TrackPoint> = None;
        let mut pf = 0u32;
        for p in &self.points {
            let (mask, fields) = match prev {
                None => (0x1Fu8, vec![p.map, p.x, p.y, p.facing, p.in_battle]),
                Some(l) => {
                    let mut m = 0u8;
                    let mut f = Vec::with_capacity(5);
                    for (i, (a, b)) in [
                        (l.map, p.map),
                        (l.x, p.x),
                        (l.y, p.y),
                        (l.facing, p.facing),
                        (l.in_battle, p.in_battle),
                    ]
                    .iter()
                    .enumerate()
                    {
                        if a != b {
                            m |= 1 << i;
                            f.push(*b);
                        }
                    }
                    (m, f)
                }
            };
            out.push(mask);
            put_varint(&mut out, p.frame.saturating_sub(pf));
            out.extend_from_slice(&fields);
            prev = Some(*p);
            pf = p.frame;
        }
        out
    }

    /// Read a `GBTK` file back.
    ///
    /// Every failure names what was wrong rather than returning an empty track:
    /// a track that silently decodes to nothing is a film of an empty world,
    /// which reads as a broken renderer.
    pub fn decode(b: &[u8]) -> Result<Track, String> {
        if b.len() < HEADER {
            return Err(format!("too short to be a track: {} byte(s)", b.len()));
        }
        if b[..4] != MAGIC {
            return Err("not a GBTK track (bad magic)".into());
        }
        if b[4] != VERSION {
            return Err(format!(
                "track version {} — this build reads {VERSION}",
                b[4]
            ));
        }
        if b[5] != 0 {
            return Err(format!("track flags {:#04x} are not understood", b[5]));
        }
        let count = u32::from_le_bytes([b[6], b[7], b[8], b[9]]) as usize;
        let last_frame = u32::from_le_bytes([b[10], b[11], b[12], b[13]]);
        let mut at = HEADER;
        let mut points = Vec::with_capacity(count);
        let mut cur = TrackPoint::default();
        for i in 0..count {
            if at >= b.len() {
                return Err(format!("ran out of bytes at record {i} of {count}"));
            }
            let mask = b[at];
            at += 1;
            if mask & 0xE0 != 0 {
                return Err(format!("record {i} sets reserved mask bits {mask:#04x}"));
            }
            if i == 0 && mask != 0x1F {
                return Err("the first record must carry every field".into());
            }
            let (d, n) = get_varint(&b[at..])
                .ok_or_else(|| format!("record {i}: truncated frame delta"))?;
            at += n;
            cur.frame = cur.frame.saturating_add(d);
            for bit in 0..5u8 {
                if mask & (1 << bit) == 0 {
                    continue;
                }
                if at >= b.len() {
                    return Err(format!("record {i}: truncated fields"));
                }
                let v = b[at];
                at += 1;
                match bit {
                    0 => cur.map = v,
                    1 => cur.x = v,
                    2 => cur.y = v,
                    3 => cur.facing = v,
                    _ => cur.in_battle = v,
                }
            }
            points.push(cur);
        }
        Ok(Track { points, last_frame })
    }
}

impl FromIterator<TrackPoint> for Track {
    fn from_iter<I: IntoIterator<Item = TrackPoint>>(it: I) -> Track {
        let mut t = Track::new();
        for p in it {
            t.push(p);
        }
        t
    }
}

fn put_varint(out: &mut Vec<u8>, mut n: u32) {
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

fn get_varint(b: &[u8]) -> Option<(u32, usize)> {
    let mut n = 0u32;
    for (i, byte) in b.iter().take(5).enumerate() {
        n |= ((byte & 0x7F) as u32) << (7 * i);
        if byte & 0x80 == 0 {
            return Some((n, i + 1));
        }
    }
    None
}

/// What a corpus of `runs` runs of this shape will cost on disk, in bytes.
///
/// Takes the measured 3.2 bytes a record so a caller can say the number before
/// it writes anything, the way `screens::estimate` does.
pub fn estimate(records_per_run: usize, runs: usize) -> u64 {
    (HEADER as u64 + (records_per_run as f64 * 3.2) as u64) * runs as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(frame: u32, map: u8, x: u8, y: u8, facing: u8, in_battle: u8) -> TrackPoint {
        TrackPoint {
            frame,
            map,
            x,
            y,
            facing,
            in_battle,
        }
    }

    #[test]
    fn a_track_round_trips_through_its_own_bytes() {
        let mut t = Track::new();
        for i in 0..500u32 {
            t.push(pt(
                i * 17,
                (i / 40) as u8,
                (i % 30) as u8,
                (i % 23) as u8,
                [0u8, 4, 8, 12][(i % 4) as usize],
                if i % 71 == 0 { 1 } else { 0 },
            ));
        }
        let back = Track::decode(&t.encode()).expect("decode");
        assert_eq!(back, t);
        assert_eq!(back.last_frame(), t.last_frame());
    }

    /// The whole size argument in one assertion: a record is a few bytes, not
    /// a line of ASCII. Teeth — widen the mask to "always five fields" and this
    /// goes red.
    #[test]
    fn a_record_costs_about_three_bytes() {
        let mut t = Track::new();
        // A walk east: one coordinate moves, nothing else.
        for i in 0..1000u32 {
            t.push(pt(i * 20, 3, (i % 40) as u8, 7, 12, 0));
        }
        let n = t.encode().len() - HEADER;
        assert!(
            n as f64 / t.len() as f64 <= 3.5,
            "{} bytes for {} records",
            n,
            t.len()
        );
    }

    /// **The reduction is on the way in.** A run that stands still for a
    /// hundred thousand frames must not hold a hundred thousand points.
    #[test]
    fn standing_still_costs_one_point_and_still_advances_the_clock() {
        let mut t = Track::new();
        for i in 0..100_000u32 {
            t.push(pt(i, 40, 5, 6, 4, 0));
        }
        assert_eq!(t.len(), 1);
        assert_eq!(t.last_frame(), 99_999);
        assert_eq!(Track::decode(&t.encode()).unwrap().last_frame(), 99_999);
    }

    /// `wIsInBattle` is a raw byte and `$FF` means blacked out. It has to
    /// survive the round trip as itself, not as a boolean — the fifth
    /// occurrence of that rule in this repository.
    #[test]
    fn a_blackout_sentinel_survives_encoding() {
        let mut t = Track::new();
        t.push(pt(0, 40, 1, 2, 0, 0));
        t.push(pt(10, 40, 1, 2, 0, 1));
        t.push(pt(20, 40, 1, 2, 0, 0xFF));
        let back = Track::decode(&t.encode()).unwrap();
        assert_eq!(
            back.points()
                .iter()
                .map(|p| p.in_battle)
                .collect::<Vec<_>>(),
            vec![0, 1, 0xFF]
        );
    }

    #[test]
    fn a_damaged_track_is_refused_rather_than_decoded_empty() {
        let mut t = Track::new();
        t.push(pt(0, 1, 2, 3, 4, 0));
        t.push(pt(9, 1, 3, 3, 12, 0));
        let good = t.encode();
        assert!(Track::decode(&good[..HEADER - 1]).is_err(), "short header");
        let mut bad = good.clone();
        bad[0] = b'X';
        assert!(Track::decode(&bad).is_err(), "bad magic");
        let mut bad = good.clone();
        bad[4] = 99;
        assert!(Track::decode(&bad).is_err(), "wrong version");
        assert!(
            Track::decode(&good[..good.len() - 1]).is_err(),
            "truncated body"
        );
    }

    #[test]
    fn an_empty_track_is_a_valid_file() {
        let t = Track::new();
        let back = Track::decode(&t.encode()).unwrap();
        assert!(back.is_empty());
        assert_eq!(back.last_frame(), 0);
    }
}
