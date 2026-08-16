//! Declarative memory watches — **for reporting only**.
//!
//! # The separation this module exists to keep
//!
//! A watch samples a named address out of the running machine and puts the
//! number in the journal and the report. It is **never** shown to the agent.
//!
//! That is not tidiness, it is what makes an evaluation mean anything. An agent
//! told "the opponent has 4 HP left" is not solving the problem the screen
//! poses; it is reading the answer out of RAM. If watches leaked into the
//! snapshot, every episode success rate this harness ever produced would be
//! uninterpretable, and — worse — the leak would be invisible in the results.
//!
//! So the two paths are structurally separate and stay that way:
//!
//! | path | reads | goes to |
//! |---|---|---|
//! | the agent's | its own `Gen1Observer` + the screen, inside the plugin | the model |
//! | a watch | this module, inside the **host** | `events.ndjson`, `episodes.ndjson`, the report |
//!
//! Nothing in `src/longplay` ever hands a [`Sample`] to a plugin, and
//! `testharness/longplay.rs` asserts it by grepping every request the driver
//! was actually given for the watch's name and value.
//!
//! # Addresses are reused, not rediscovered
//!
//! [`Watch::named`] resolves a handful of human names to the addresses
//! [`terminalgb_plugin_trainer::gen1`] has already derived and verified. A raw
//! `0xXXXX` is accepted too, because a person watching a hack or a different
//! game needs one — but the named form is what a shared episode file should
//! use, since a name survives a re-derivation and a number does not.

use terminalgb_plugin_agent::json;
use terminalgb_plugin_api::Machine;
use terminalgb_plugin_trainer::gen1::addr as tr;

/// One address to sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Watch {
    pub name: String,
    pub addr: u16,
    /// 1, 2 or 4 bytes.
    pub len: u8,
    /// Gen 1 multi-byte fields are big-endian; that is the default here for
    /// exactly that reason.
    pub big_endian: bool,
    /// Bits to keep, ANDed with the value read. `0` means "the whole value",
    /// which is what every watch written before this existed meant.
    ///
    /// **This is what makes a single event flag watchable.** Gen 1's story is
    /// 2,560 single-bit events packed into the 320 bytes at `wEventFlags`
    /// ($D747), so eight unrelated milestones share one byte: `$D74B` alone
    /// carries "Oak asked you to choose", "you took a starter", "you fought the
    /// rival" and "you were given the Pokédex". A byte comparison cannot name
    /// one of them — `>= 32` is true for four different reasons — so without a
    /// mask a goal about an event flag is a goal about something else. With one
    /// it is exact: `{"name":"got_pokedex","addr":"0xD74B","mask":"0x20"}` and
    /// the condition `watch:got_pokedex!=0`.
    pub mask: u32,
}

/// The built-in names, each pointing at an address another crate derived.
///
/// Kept short on purpose. Anything a run reports on *routinely* — position,
/// badges, party, money, the Pokédex — is in [`super::progress::Progress`]
/// already and needs no watch; these are the extras an episode about a
/// specific problem wants.
pub const NAMED: &[(&str, u16, u8, bool)] = &[
    ("in_battle", tr::IS_IN_BATTLE, 1, false),
    ("battle_type", tr::BATTLE_TYPE, 1, false),
    ("cur_opponent", tr::CUR_OPPONENT, 1, false),
    ("enemy_species", tr::ENEMY_MON, 1, false),
    // `enemy_mon::HP` is offset 1 into `wEnemyMon`, big-endian.
    ("enemy_hp", tr::ENEMY_MON + 1, 2, true),
    ("enemy_level", tr::ENEMY_MON + 14, 1, false),
    ("trainer_class", tr::TRAINER_CLASS, 1, false),
    // `party_struct::HP` is offset 1 into `wPartyMon1`, big-endian.
    ("lead_hp", tr::PARTY_MONS + 1, 2, true),
    ("lead_max_hp", tr::PARTY_MONS + 34, 2, true),
    ("lead_level", tr::PARTY_MONS + 33, 1, false),
    ("lead_status", tr::PARTY_MONS + 4, 1, false),
];

impl Watch {
    /// Resolve a built-in name.
    pub fn named(name: &str) -> Option<Watch> {
        NAMED
            .iter()
            .find(|(n, ..)| *n == name)
            .map(|(n, addr, len, be)| Watch {
                name: (*n).to_string(),
                addr: *addr,
                len: *len,
                big_endian: *be,
                mask: 0,
            })
    }

    /// Parse one watch out of an episode file's `watch` array.
    ///
    /// Two forms: `"enemy_hp"` (a built-in) or
    /// `{"name":"flag","addr":"0xD747","len":1,"mask":"0x01"}`.
    pub fn from_json(v: &json::Json) -> Result<Watch, String> {
        if let Some(s) = v.as_str() {
            return Watch::named(s).ok_or_else(|| {
                format!(
                    "unknown watch {s:?} — known names are {}",
                    NAMED
                        .iter()
                        .map(|(n, ..)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            });
        }
        let name = v
            .str_field("name")
            .ok_or("a watch needs a name")?
            .to_string();
        if name.is_empty() {
            return Err("a watch's name may not be empty".into());
        }
        let addr = match v.get("addr") {
            Some(a) => parse_addr(a)?,
            None => {
                return Watch::named(&name).ok_or(format!(
                    "watch {name:?} has no addr and is not a known name"
                ))
            }
        };
        let len = v.u64_field("len").unwrap_or(1) as u8;
        if !matches!(len, 1 | 2 | 4) {
            return Err(format!("watch {name:?}: len must be 1, 2 or 4, not {len}"));
        }
        let mask = match v.get("mask") {
            Some(m) => parse_mask(m).map_err(|e| format!("watch {name:?}: {e}"))?,
            None => 0,
        };
        // A mask outside the bytes actually read can only ever report zero, and
        // a goal that can never be true is the expensive mistake this whole
        // module is careful about. Name it here rather than at attempt 1,000.
        let width = 8 * len as u32;
        if mask != 0 && width < 32 && mask >> width != 0 {
            return Err(format!(
                "watch {name:?}: mask {mask:#X} has bits above the {len} byte(s) it \
                 reads, so it could only ever be zero"
            ));
        }
        Ok(Watch {
            name,
            addr,
            len,
            big_endian: v
                .get("big_endian")
                .and_then(|b| b.as_bool())
                .unwrap_or(true),
            mask,
        })
    }

    pub fn to_json(&self, w: &mut json::Writer) {
        w.begin_obj();
        w.str_field("name", &self.name);
        w.str_field("addr", &format!("{:#06X}", self.addr));
        w.num_field("len", self.len as f64);
        w.bool_field("big_endian", self.big_endian);
        if self.mask != 0 {
            w.str_field("mask", &format!("{:#04X}", self.mask));
        }
        w.end_obj();
    }

    fn read(&self, m: &mut Machine) -> u32 {
        let mut bytes = [0u8; 4];
        m.peek_into(self.addr, &mut bytes[..self.len as usize]);
        let mut v = 0u32;
        if self.big_endian {
            for b in &bytes[..self.len as usize] {
                v = (v << 8) | *b as u32;
            }
        } else {
            for b in bytes[..self.len as usize].iter().rev() {
                v = (v << 8) | *b as u32;
            }
        }
        if self.mask != 0 {
            v &= self.mask;
        }
        v
    }
}

/// `"0x20"`, `"$20"` or `32`. Shares the address parser's refusal of a bare
/// `"20"`: decimal or hex is not a guess worth making about a bit pattern.
fn parse_mask(v: &json::Json) -> Result<u32, String> {
    if let Some(n) = v.as_u64() {
        return u32::try_from(n).map_err(|_| format!("{n} is not a 32-bit mask"));
    }
    let s = v
        .as_str()
        .ok_or("mask must be a number or a string")?
        .trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .or_else(|| s.strip_prefix('$'));
    match hex {
        Some(h) => u32::from_str_radix(h, 16).map_err(|_| format!("{s:?} is not a mask")),
        None => s.parse().map_err(|_| format!("{s:?} is not a mask")),
    }
}

/// `"0xD747"`, `"$D747"` or `53575`.
fn parse_addr(v: &json::Json) -> Result<u16, String> {
    if let Some(n) = v.as_u64() {
        return u16::try_from(n).map_err(|_| format!("{n} is not a 16-bit address"));
    }
    let s = v
        .as_str()
        .ok_or("addr must be a number or a string")?
        .trim();
    let hex = s
        .strip_prefix("0x")
        .or_else(|| s.strip_prefix("0X"))
        .or_else(|| s.strip_prefix('$'));
    match hex {
        Some(h) => {
            u16::from_str_radix(h, 16).map_err(|_| format!("{s:?} is not an address"))
        }
        None => s.parse().map_err(|_| format!("{s:?} is not an address")),
    }
}

/// The watch list of one run or episode.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Watches(pub Vec<Watch>);

impl Watches {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Read every watch. ~1 bus read per byte watched.
    pub fn read(&self, m: &mut Machine) -> Sample {
        Sample(self.0.iter().map(|w| (w.name.clone(), w.read(m))).collect())
    }

    pub fn get(&self, name: &str) -> Option<&Watch> {
        self.0.iter().find(|w| w.name == name)
    }
}

/// One reading of every watch. **Reporting only** — see the module docs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sample(pub Vec<(String, u32)>);

impl Sample {
    pub fn get(&self, name: &str) -> Option<u32> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| *v)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Have any of the watched values moved?
    pub fn differs(&self, other: &Sample) -> bool {
        self != other
    }

    pub fn write_json(&self, w: &mut json::Writer) {
        w.begin_obj();
        for (name, value) in &self.0 {
            w.num_field(name, *value as f64);
        }
        w.end_obj();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_built_in_name_resolves_to_an_address_another_crate_derived() {
        let w = Watch::named("enemy_hp").expect("known");
        assert_eq!(w.addr, tr::ENEMY_MON + 1);
        assert_eq!(w.len, 2);
        assert!(w.big_endian, "every multi-byte Gen 1 field is big-endian");
        assert!(Watch::named("no_such_thing").is_none());
    }

    #[test]
    fn an_unknown_name_is_refused_with_the_list_of_known_ones() {
        let v = json::parse("\"nonsense\"").unwrap();
        let e = Watch::from_json(&v).unwrap_err();
        assert!(e.contains("enemy_hp"), "{e}");
    }

    #[test]
    fn a_raw_address_is_accepted_in_every_spelling_a_person_writes() {
        for spelling in ["\"0xD747\"", "\"$D747\"", "55111"] {
            let text = format!(r#"{{"name":"flag","addr":{spelling},"len":1}}"#);
            let v = json::parse(&text).unwrap();
            let w = Watch::from_json(&v).expect(spelling);
            assert_eq!(w.addr, 0xD747, "{spelling}");
        }
        // A bare "D747" is ambiguous — decimal or hex? — so it is refused
        // rather than guessed at. An address read wrong is a watch that
        // reports a plausible number about the wrong byte for ever.
        let v = json::parse(r#"{"name":"flag","addr":"D747","len":1}"#).unwrap();
        assert!(Watch::from_json(&v).is_err());
    }

    #[test]
    fn a_silly_length_is_refused_rather_than_read_as_garbage() {
        let v = json::parse(r#"{"name":"x","addr":"0xC000","len":3}"#).unwrap();
        assert!(Watch::from_json(&v).unwrap_err().contains("1, 2 or 4"));
    }

    #[test]
    fn a_watch_round_trips_through_its_own_json() {
        let w = Watch {
            name: "flag".into(),
            addr: 0xD747,
            len: 2,
            big_endian: false,
            mask: 0x0140,
        };
        let mut out = json::Writer::new();
        w.to_json(&mut out);
        let back = Watch::from_json(&json::parse(&out.finish()).unwrap()).unwrap();
        assert_eq!(w, back);
    }

    /// Eight unrelated milestones share `$D74B`, so a byte comparison cannot
    /// name one of them. The mask is what turns "the story so far" into a
    /// single checkable fact — which is the whole reason a scenario's goal may
    /// be an event flag rather than a coordinate somebody guessed.
    #[test]
    fn a_mask_isolates_one_bit_of_a_byte_that_holds_eight_events() {
        let v = json::parse(
            r#"{"name":"got_pokedex","addr":"0xD74B","len":1,"mask":"0x20"}"#,
        )
        .unwrap();
        let w = Watch::from_json(&v).unwrap();
        assert_eq!(w.mask, 0x20);
        // EVENT_GOT_STARTER (0x04) and EVENT_BATTLED_RIVAL_IN_OAKS_LAB (0x08)
        // are set and the Pokédex is not: the byte reads 0x0C, which is >= 4
        // and would satisfy any plain comparison a person might reach for.
        assert_eq!(0x0C_u32 & w.mask, 0, "the dex bit is not set yet");
        assert_eq!(0x2C_u32 & w.mask, 0x20, "now it is");
    }

    /// A mask nothing could ever satisfy is the expensive mistake — every
    /// attempt times out and the bill is real. Refused where it is written.
    #[test]
    fn a_mask_above_the_bytes_it_reads_is_refused_rather_than_always_zero() {
        let v = json::parse(r#"{"name":"x","addr":"0xD74B","len":1,"mask":"0x100"}"#)
            .unwrap();
        assert!(Watch::from_json(&v)
            .unwrap_err()
            .contains("could only ever be zero"));
        // Two bytes really can carry it.
        let v = json::parse(r#"{"name":"x","addr":"0xD74B","len":2,"mask":"0x100"}"#)
            .unwrap();
        assert_eq!(Watch::from_json(&v).unwrap().mask, 0x100);
    }

    /// Every watch written before the mask existed means "the whole value",
    /// and must keep meaning it.
    #[test]
    fn a_watch_with_no_mask_reads_the_whole_value_as_it_always_did() {
        let v = json::parse(r#"{"name":"x","addr":"0xD163","len":1}"#).unwrap();
        assert_eq!(Watch::from_json(&v).unwrap().mask, 0);
        assert_eq!(Watch::named("enemy_hp").unwrap().mask, 0);
    }

    /// A bare `"20"` is decimal or hex and guessing is worse than refusing —
    /// the same rule the address parser already applies, for the same reason.
    #[test]
    fn a_bare_mask_is_refused_and_every_spelled_form_is_accepted() {
        for (spelling, expect) in [("\"0x20\"", 0x20), ("\"$20\"", 0x20), ("32", 32)] {
            let text = format!(r#"{{"name":"x","addr":"0xD74B","mask":{spelling}}}"#);
            let v = json::parse(&text).unwrap();
            assert_eq!(Watch::from_json(&v).unwrap().mask, expect, "{spelling}");
        }
        let v = json::parse(r#"{"name":"x","addr":"0xD74B","mask":"20"}"#).unwrap();
        assert!(Watch::from_json(&v).unwrap().mask == 20);
        let v = json::parse(r#"{"name":"x","addr":"0xD74B","mask":"zz"}"#).unwrap();
        assert!(Watch::from_json(&v).is_err());
    }

    #[test]
    fn a_sample_is_a_plain_name_to_number_map() {
        let s = Sample(vec![("hp".into(), 21), ("lvl".into(), 7)]);
        assert_eq!(s.get("hp"), Some(21));
        assert_eq!(s.get("nope"), None);
        let mut w = json::Writer::new();
        s.write_json(&mut w);
        let text = w.finish();
        assert!(text.contains("\"hp\":21"), "{text}");
        assert!(s.differs(&Sample::default()));
    }
}
