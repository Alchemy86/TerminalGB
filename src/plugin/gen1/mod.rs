//! **The first plugin: a virtual link-cable partner for Generation 1.**
//!
//! It emulates the second Game Boy on the serial link, so Pokémon Red/Blue/Yellow
//! believes it is talking to a real trade partner at the Cable Club, and completes
//! a genuine trade.
//!
//! # What it deliberately does *not* do
//!
//! It does not write memory, patch the ROM, or place a Pokémon into the party.
//! It declares [`Capability::SERIAL_LINK`] and [`Capability::READ_MEMORY`] and
//! **not** [`Capability::WRITE_MEMORY`], so the plugin host would refuse a write
//! even if the code attempted one. The Pokémon arrives the way any traded
//! Pokémon arrives: the game asks for it over the wire and inserts it itself.
//! That is what makes this a test of link trading rather than a cheat — and it
//! is why the received Pokémon has to be genuinely well formed (see
//! [`party`]), because the game will not fix anything up afterwards.
//!
//! # How to use it
//!
//! Enable it with `plugins = gen1-link` in the terminal frontend's
//! `config.toml`, or `GB_PLUGINS=gen1-link`. Point it at a trade file with
//! `gen1_trade = <path>` / `GB_GEN1_TRADE=<path>`; with no file it offers the
//! level 5 event Mew ([`spec::TradeSpec::default`]). Then, in game: Pokémon
//! Center → upstairs → talk to the link receptionist → save → **TRADE CENTER** →
//! walk to the table and press A.
//!
//! # Layers
//!
//! * [`text`] — the Gen 1 character encoding names are stored in.
//! * [`romdata`] — base stats, species and move tables, read out of the player's
//!   own cartridge rather than embedded here.
//! * [`party`] — the 44-byte Pokémon structure, the 418-byte party block, and
//!   the patch list.
//! * [`protocol`] — the byte-level link state machine.
//! * [`spec`] — [`PokemonSource`](spec::PokemonSource): *where the Pokémon comes
//!   from*, kept separate from how it is traded.

pub mod party;
pub mod protocol;
pub mod romdata;
pub mod spec;
pub mod text;

use super::{Capability, Event, Machine, Plugin};
use protocol::LinkPartner;
use romdata::RomTables;
use spec::{PokemonSource, SourceError, TradeSpec};

/// The plugin's registered name.
pub const NAME: &str = "gen1-link";

/// A virtual second Game Boy running Pokémon Red/Blue/Yellow.
pub struct Gen1LinkPartner {
    partner: LinkPartner,
    offer: String,
    origin: String,
    /// Sticky log of what happened, so a headless run can assert on it and the
    /// TUI can toast it.
    events: Vec<String>,
    last_trades: u32,
    /// Ring buffer of the last [`TRACE_LEN`] `(sent, replied, phase after)`
    /// triples, enabled by `GB_GEN1_LINK_TRACE=1`.
    ///
    /// The link is thousands of bytes long and every interesting failure is a
    /// *reply to one byte*, so the only way to diagnose one honestly is to look
    /// at the wire. Keeping the tail is enough: the close sequence, the
    /// selection round and the post-trade sync all sit at the end of a run.
    trace: Option<std::collections::VecDeque<(u8, u8, protocol::Phase)>>,
}

/// How many wire bytes the optional trace retains.
const TRACE_LEN: usize = 4096;

impl Gen1LinkPartner {
    /// Build the partner from a ROM image and a source of Pokémon.
    ///
    /// The ROM image is needed (not just borrowed memory) because the species,
    /// base-stat and move tables live in banks the running game may not have
    /// mapped — see [`romdata`].
    pub fn new(
        rom: Vec<u8>,
        source: &dyn PokemonSource,
    ) -> Result<Gen1LinkPartner, SourceError> {
        let tables = RomTables::locate(rom)?;
        let party = source.party(&tables)?;
        let offer = describe_party(&party, &tables);
        let partner = LinkPartner::new(party)?;
        Ok(Gen1LinkPartner {
            partner,
            offer,
            origin: source.describe(),
            events: Vec::new(),
            last_trades: 0,
            trace: std::env::var("GB_GEN1_LINK_TRACE")
                .is_ok_and(|v| v != "0")
                .then(|| std::collections::VecDeque::with_capacity(TRACE_LEN)),
        })
    }

    /// Convenience: build from a ROM and an optional trade-file path, falling
    /// back to the built-in event Mew.
    pub fn from_trade_file(
        rom: Vec<u8>,
        path: Option<&std::path::Path>,
    ) -> Result<Gen1LinkPartner, SourceError> {
        let spec = match path {
            Some(p) => TradeSpec::load(p)?,
            None => TradeSpec::default(),
        };
        Gen1LinkPartner::new(rom, &spec)
    }

    /// One-line description of what is on offer, e.g. `MEW (Lv5)`.
    pub fn offer(&self) -> &str {
        &self.offer
    }

    pub fn trades_completed(&self) -> u32 {
        self.partner.trades_completed()
    }

    pub fn exchanges_completed(&self) -> u32 {
        self.partner.exchanges_completed()
    }

    pub fn phase(&self) -> protocol::Phase {
        self.partner.phase()
    }

    /// The party the game last sent us — proof the exchange really happened,
    /// and what a bridge would forward on.
    pub fn seen_party(&self) -> Option<&party::Party> {
        self.partner.seen_party()
    }

    /// Human-readable log of protocol milestones.
    pub fn log(&self) -> &[String] {
        &self.events
    }

    /// The tail of the wire, when `GB_GEN1_LINK_TRACE=1` was set at build time
    /// of the plugin: `(byte the game sent, byte we replied, phase afterwards)`.
    pub fn trace(&self) -> Vec<(u8, u8, protocol::Phase)> {
        self.trace
            .as_ref()
            .map(|t| t.iter().copied().collect())
            .unwrap_or_default()
    }

    /// Where the offered Pokémon came from — the [`PokemonSource`] description.
    /// Surfaced so a user can tell an event distribution from a file they wrote
    /// from a remote partner.
    pub fn origin(&self) -> &str {
        &self.origin
    }
}

fn describe_party(p: &party::Party, tables: &RomTables) -> String {
    match p.mons.first() {
        Some(m) => {
            let name = tables
                .species_by_index(m.species)
                .map(|i| i.name)
                .unwrap_or_else(|_| format!("#{}", m.species));
            if m.nickname.is_empty() || m.nickname == name {
                format!("{name} (Lv{})", m.level)
            } else {
                format!("{} the {name} (Lv{})", m.nickname, m.level)
            }
        }
        None => "nothing".into(),
    }
}

impl Plugin for Gen1LinkPartner {
    fn name(&self) -> &str {
        NAME
    }

    fn capabilities(&self) -> Capability {
        // Note the absence of WRITE_MEMORY: this plugin cannot poke the game
        // even by accident. READ_MEMORY is declared for diagnostics only.
        Capability::SERIAL_LINK | Capability::OBSERVE | Capability::READ_MEMORY
    }

    fn status(&self) -> String {
        let n = self.partner.trades_completed();
        if n > 0 {
            format!("offering {} — {n} traded", self.offer)
        } else {
            format!("offering {} — {}", self.offer, self.partner.phase().label())
        }
    }

    fn on_serial(&mut self, sent: u8, _machine: &mut Machine) -> Option<u8> {
        let before = self.partner.phase();
        let reply = self.partner.feed(sent);
        let after = self.partner.phase();
        if let Some(t) = self.trace.as_mut() {
            if t.len() == TRACE_LEN {
                t.pop_front();
            }
            t.push_back((sent, reply, after));
        }
        if before != after {
            self.events
                .push(format!("{} -> {}", before.label(), after.label()));
        }
        if self.partner.trades_completed() != self.last_trades {
            self.last_trades = self.partner.trades_completed();
            self.events.push(format!(
                "trade #{} confirmed: sent {}",
                self.last_trades, self.offer
            ));
        }
        Some(reply)
    }

    fn on_event(&mut self, _event: &Event, _machine: &mut Machine) {}

    fn reset(&mut self) {
        self.partner.reset();
        self.events.push("reset (save state loaded)".into());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM image containing all five tables at plausible offsets, so the
    /// locator and the stat maths can be tested without shipping a cartridge.
    fn synthetic_rom() -> Vec<u8> {
        let mut rom = vec![0u8; 0x60000];
        // Fill with a value that cannot collide with any signature.
        for (i, b) in rom.iter_mut().enumerate() {
            *b = ((i * 7 + 3) % 23) as u8 + 0x01;
        }

        // --- BaseStats: 150 entries of 28 bytes, Pokédex order.
        let base = 0x10000;
        for dex in 1..=150usize {
            let e = base + (dex - 1) * 28;
            rom[e] = dex as u8;
            rom[e + 1] = 45; // hp
            rom[e + 2] = 49;
            rom[e + 3] = 49;
            rom[e + 4] = 45;
            rom[e + 5] = 65;
            rom[e + 6] = 0x16;
            rom[e + 7] = 0x03;
            rom[e + 8] = 45;
            rom[e + 9] = 64;
            rom[e + 19] = 0; // growth rate: medium fast
            for b in rom.iter_mut().take(e + 28).skip(e + 20) {
                *b = 0;
            }
        }
        // Pikachu is dex 25 and is the cross-check entry.
        let p = base + 24 * 28;
        rom[p + 1] = 35;
        rom[p + 2] = 55;
        rom[p + 3] = 30;
        rom[p + 4] = 90;
        rom[p + 5] = 50;
        rom[p + 6] = 23;
        rom[p + 7] = 23;
        rom[p + 8] = 190;

        // --- MewBaseStats, stored apart as in Red/Blue.
        let mew = 0x08000;
        rom[mew..mew + 28].fill(0);
        rom[mew] = 151;
        for i in 1..6 {
            rom[mew + i] = 100;
        }
        rom[mew + 6] = 0x18;
        rom[mew + 7] = 0x18;
        rom[mew + 8] = 45;
        rom[mew + 9] = 64;

        // --- PokedexOrder: internal index -> dex.
        let order = 0x20000;
        rom[order..order + 190].fill(0);
        let head = [112u8, 115, 32, 35, 21, 100];
        rom[order..order + 6].copy_from_slice(&head);
        // internal 0x54 must be Pikachu (dex 25); 0x15 must be Mew (dex 151).
        rom[order + 0x54 - 1] = 25;
        rom[order + 0x15 - 1] = 151;
        rom[order + 0x99 - 1] = 1;

        // --- MonsterNames: 10-byte fixed fields, internal-index order.
        let names = 0x30000;
        rom[names..names + 190 * 10].fill(text::TERMINATOR);
        let put = |rom: &mut Vec<u8>, idx: usize, s: &str| {
            let at = names + (idx - 1) * 10;
            let enc = text::encode_fixed(s, 10);
            rom[at..at + 10].copy_from_slice(&enc);
        };
        put(&mut rom, 1, "RHYDON");
        put(&mut rom, 0x54, "PIKACHU");
        put(&mut rom, 0x15, "MEW");
        put(&mut rom, 0x99, "BULBASAUR");

        // --- Moves: 6-byte records.
        let moves = 0x40000;
        rom[moves..moves + 165 * 6].fill(0);
        rom[moves..moves + 12]
            .copy_from_slice(&[1, 0, 40, 0, 255, 35, 2, 0, 50, 0, 255, 25]);
        rom[moves + 84 * 6 + 5] = 15; // move 85 (THUNDERBOLT) has 15 PP

        // --- MoveNames: terminator-separated list.
        let move_names = 0x50000;
        let mut p = move_names;
        for id in 1..=165 {
            let n = match id {
                1 => "POUND",
                2 => "KARATECHOP",
                85 => "THUNDERBOLT",
                _ => "TACKLE",
            };
            for b in n.chars().filter_map(text::encode_char) {
                rom[p] = b;
                p += 1;
            }
            rom[p] = text::TERMINATOR;
            p += 1;
        }
        rom
    }

    #[test]
    fn rom_tables_locate_and_verify() {
        let t = RomTables::locate(synthetic_rom()).expect("tables located");
        let pika = t.species_by_index(0x54).unwrap();
        assert_eq!(pika.name, "PIKACHU");
        assert_eq!(pika.dex, 25);
        assert_eq!(pika.base.speed, 90);
        let mew = t.resolve_species("Mew").unwrap();
        assert_eq!(mew.index, 0x15);
        assert_eq!(mew.base.hp, 100);
    }

    #[test]
    fn species_can_be_named_dex_numbered_or_indexed() {
        let t = RomTables::locate(synthetic_rom()).unwrap();
        assert_eq!(t.resolve_species("mew").unwrap().index, 0x15);
        assert_eq!(t.resolve_species("#151").unwrap().index, 0x15);
        assert_eq!(t.resolve_species("21").unwrap().index, 0x15);
        assert!(t.resolve_species("Mewtwo").is_err());
    }

    #[test]
    fn a_rom_without_the_tables_is_refused_rather_than_guessed_at() {
        assert!(RomTables::locate(vec![0u8; 0x8000]).is_err());
    }

    #[test]
    fn the_default_offer_builds_a_well_formed_mew() {
        let p = Gen1LinkPartner::from_trade_file(synthetic_rom(), None).unwrap();
        assert_eq!(p.offer(), "MEW (Lv5)");
        let mon = p.partner.offered().expect("an offered mon");
        assert_eq!(mon.level, 5);
        assert_eq!(mon.moves[0], 1);
        assert_eq!(mon.pp[0], 35);
        // Base 100 across the board, perfect DVs, level 5, no stat exp:
        //   HP    = ((100+15)*2)*5/100 + 5 + 10 = 11 + 15 = 26
        //   other = ((100+15)*2)*5/100 + 5      = 11 + 5  = 16
        assert_eq!(mon.stats.hp, 26);
        assert_eq!(mon.stats.attack, 16);
        assert_eq!(mon.current_hp, mon.stats.hp);
        // Medium-fast growth: level 5 => 125 experience exactly.
        assert_eq!(mon.experience, 125);
        assert_eq!(mon.ot_name, "GF");
        assert_eq!(mon.ot_id, 22796);
    }

    #[test]
    fn the_plugin_declares_no_write_capability() {
        let p = Gen1LinkPartner::from_trade_file(synthetic_rom(), None).unwrap();
        let caps = p.capabilities();
        assert!(caps.contains(Capability::SERIAL_LINK));
        assert!(!caps.contains(Capability::WRITE_MEMORY));
    }

    #[test]
    fn a_bad_species_name_is_reported_with_the_name() {
        let spec =
            spec::TradeSpec::parse("species = Charizard\nlevel = 50\n", "t").unwrap();
        let e = match Gen1LinkPartner::new(synthetic_rom(), &spec) {
            Err(e) => e,
            Ok(_) => panic!("a species not in this ROM must be refused"),
        };
        assert!(format!("{e}").contains("Charizard"), "{e}");
    }
}
