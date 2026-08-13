//! A live trainer view: the party, the badges and the battle opponent, read out
//! of a running Gen 1 Pokémon game.
//!
//! This is the second consumer of the plugin surface in [`crate::plugin`] (the
//! first being the telemetry stream), and it is deliberately **read-only**: it
//! is handed a [`crate::plugin::DebugView`], which borrows `&Gameboy`, so it is
//! statically incapable of writing to the machine. A tracker that can corrupt a
//! save is worse than no tracker.
//!
//! # Prior art
//!
//! The shape of the feature is the [Ironmon
//! Tracker](https://github.com/besteon/Ironmon-Tracker): a Lua script for
//! BizHawk/mGBA that reads party and enemy structures out of emulator memory
//! each frame and draws a panel beside the game. This is the same idea built
//! *into* the emulator rather than bolted on through a scripting bridge, which
//! removes the tracker's hardest problem — it does not have to guess when the
//! data it is reading is mid-update, because it reads between frames.
//!
//! # What it reads, and how it knows the addresses are right
//!
//! Every address is derived from a verified build of the
//! [`pret/pokered`](https://github.com/pret/pokered) disassembly whose ROM
//! SHA-1s match the retail cartridges; see [`gen1`] for the full derivation and
//! [`rom`] for the cartridge tables. Nothing is quoted from memory or from a
//! wiki.
//!
//! # Cost
//!
//! The render path was recently rescued from 18 fps and must not be given the
//! cost back. So: nothing is read per frame. The plugin samples at
//! [`SAMPLE_EVERY`] frames (about 10 Hz, faster than a human reads and far
//! slower than the game runs), and only when its view is on screen
//! ([`TrainerPlugin::set_active`]). Sprites — the only expensive part — are
//! decompressed once per species and cached, so a party that has not changed
//! costs nothing at all.
//!
//! # Scope
//!
//! Party, badges and the battle opponent. Deliberately *not* here, and named
//! rather than half-built: type-effectiveness hints against the current
//! opponent, catch-rate odds, route encounter tables, move-power/accuracy
//! display, and the run-tracking (seed counters, "furthest gym") that an Ironmon
//! run wants. Each is additive on top of this data.

pub mod derived;
pub mod gen1;
pub mod rom;
pub mod sprite;
pub mod text;

use std::collections::HashMap;

use crate::plugin::{DebugView, Event, Plugin};
use gen1::{addr, battle, party, BattleKind, Dvs, Mon, Status};

/// How often the party is re-read, in emulated frames. Six frames is about
/// 10 Hz — a tenth of a second behind the game, which no one can see, for a
/// tenth of the work.
pub const SAMPLE_EVERY: u64 = 6;

/// Which game the cartridge is, if it is one this can read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Game {
    Red,
    Blue,
}

impl Game {
    pub fn label(self) -> &'static str {
        match self {
            Game::Red => "Pokémon Red",
            Game::Blue => "Pokémon Blue",
        }
    }

    /// Identify the cartridge from its header.
    ///
    /// The title alone would also match a translation, a ROM hack or a
    /// randomiser with a moved RAM layout, all of which would render confident
    /// nonsense. So the header's global checksum must match too: these are the
    /// values in the retail ROMs whose SHA-1s are recorded in [`gen1`].
    ///
    /// Everything else — Yellow, Gen 2, and every other Game Boy game — is
    /// refused. The panel then says so, which is the whole point: a wrong
    /// address does not fail loudly, it silently shows the wrong number.
    pub fn detect(title: &str, checksum: u16) -> Option<Game> {
        match (title.trim_end_matches('\0').trim(), checksum) {
            ("POKEMON RED", 0x9120) => Some(Game::Red),
            ("POKEMON BLUE", 0x9D0A) => Some(Game::Blue),
            _ => None,
        }
    }
}

/// A decompressed front sprite, as palette indices 0..=3.
#[derive(Debug, Clone)]
pub struct SpriteImage {
    pub pixels: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// Everything the panel draws, as of the last sample.
#[derive(Debug, Clone, Default)]
pub struct TrainerState {
    /// False until a save has actually been loaded — at the title screen the
    /// party area holds whatever the boot code left there.
    pub in_game: bool,
    pub player_name: String,
    pub player_id: u16,
    pub money: u32,
    /// Hours, minutes, seconds.
    pub play_time: (u8, u8, u8),
    /// `wObtainedBadges`, one bit per badge.
    pub badges: u8,
    pub party: Vec<Mon>,
    pub battle: BattleKind,
    /// The opposing Pokémon, present only during a battle.
    pub opponent: Option<Mon>,
    /// The player's active Pokémon as the battle engine sees it — its stats
    /// here include in-battle modifiers, which the party copy does not.
    pub active: Option<Mon>,
    /// The opposing trainer's name, in a trainer battle.
    pub trainer_name: String,
}

impl TrainerState {
    /// One flag per badge, in `wObtainedBadges` bit order.
    pub fn badge_flags(&self) -> [bool; 8] {
        let mut out = [false; 8];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.badges & (1 << i) != 0;
        }
        out
    }

    pub fn badge_count(&self) -> u32 {
        self.badges.count_ones()
    }
}

/// The plugin.
pub struct TrainerPlugin {
    game: Option<Game>,
    tables: rom::RomTables,
    state: TrainerState,
    /// Species → decompressed sprite. Bounded by the 190 internal indices, and
    /// in practice by however many distinct Pokémon a player meets.
    sprites: HashMap<u8, Option<SpriteImage>>,
    /// Whether the frontend is currently showing the view. When it is not, the
    /// plugin does no work at all.
    active: bool,
    last_sample: u64,
    /// Set once by [`TrainerPlugin::on_attach`] so the frontend can explain the
    /// refusal instead of showing an empty panel.
    detected_title: String,
}

impl TrainerPlugin {
    /// Build a plugin against a cartridge image.
    ///
    /// The ROM is passed in rather than read through the view because
    /// [`DebugView`] cannot read a bank the game does not currently have
    /// mapped; see the note at the top of [`crate::plugin`].
    pub fn new(rom: Vec<u8>) -> TrainerPlugin {
        TrainerPlugin {
            game: None,
            tables: rom::RomTables::new(rom),
            state: TrainerState::default(),
            sprites: HashMap::new(),
            active: false,
            last_sample: 0,
            detected_title: String::new(),
        }
    }

    /// The game, or `None` when the cartridge is not one this can read.
    pub fn game(&self) -> Option<Game> {
        self.game
    }

    pub fn state(&self) -> &TrainerState {
        &self.state
    }

    /// Why the panel is empty, when it is.
    pub fn refusal(&self) -> Option<String> {
        if self.game.is_some() {
            return None;
        }
        Some(format!(
            "The trainer view reads Pokémon Red/Blue (English). \
             This cartridge is \"{}\", so its memory layout is unknown and \
             nothing is shown rather than the wrong numbers.",
            self.detected_title
        ))
    }

    /// Tell the plugin whether its view is on screen. A hidden view costs
    /// nothing beyond the frame counter.
    pub fn set_active(&mut self, active: bool) {
        self.active = active;
        if active {
            // Re-read immediately on the next event rather than waiting out the
            // sampling interval, so opening the panel is not visibly stale.
            self.last_sample = 0;
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The cached sprite for a species, decompressing it on first use.
    pub fn sprite(&mut self, species: u8) -> Option<&SpriteImage> {
        #[allow(clippy::map_entry)] // the value is built from &self.tables, so
        // the entry API's closure would borrow self twice.
        if !self.sprites.contains_key(&species) {
            let decoded =
                self.tables
                    .front_sprite(species)
                    .map(|(pixels, w, h)| SpriteImage {
                        pixels,
                        width: w,
                        height: h,
                    });
            self.sprites.insert(species, decoded);
        }
        self.sprites.get(&species).and_then(|s| s.as_ref())
    }

    /// Read the whole state. Called at [`SAMPLE_EVERY`], never per frame.
    fn sample(&mut self, view: &DebugView<'_>) {
        let mut st = TrainerState::default();

        let mut name = [0u8; gen1::NAME_LENGTH as usize];
        view.peek_into(addr::PLAYER_NAME, &mut name);
        let count = view.peek(addr::PARTY_COUNT);

        // Before a save is loaded the party region is uninitialised. A valid
        // player name plus a party count in range is the cheap, reliable
        // "the game is actually running a save" test.
        st.in_game =
            text::looks_like_a_name(&name) && count as usize <= gen1::PARTY_LENGTH;
        if !st.in_game {
            self.state = st;
            return;
        }

        st.player_name = text::decode(&name);
        st.player_id = view.peek_be16(addr::PLAYER_ID);
        st.money = text::bcd3([
            view.peek(addr::PLAYER_MONEY),
            view.peek(addr::PLAYER_MONEY + 1),
            view.peek(addr::PLAYER_MONEY + 2),
        ]);
        st.play_time = (
            view.peek(addr::PLAY_TIME_HOURS),
            view.peek(addr::PLAY_TIME_MINUTES),
            view.peek(addr::PLAY_TIME_SECONDS),
        );
        st.badges = view.peek(addr::OBTAINED_BADGES);

        for i in 0..count as u16 {
            let base = addr::PARTY_MONS + i * gen1::PARTY_MON_SIZE;
            let mut nick = [0u8; gen1::NAME_LENGTH as usize];
            view.peek_into(addr::PARTY_MON_NICKS + i * gen1::NAME_LENGTH, &mut nick);
            st.party.push(self.read_party_mon(view, base, &nick));
        }

        st.battle = BattleKind::from_flag(view.peek(addr::IS_IN_BATTLE));
        if st.battle != BattleKind::None {
            let mut nick = [0u8; gen1::NAME_LENGTH as usize];
            view.peek_into(addr::ENEMY_MON_NICK, &mut nick);
            st.opponent = Some(self.read_battle_mon(view, addr::ENEMY_MON, &nick));

            let mut nick = [0u8; gen1::NAME_LENGTH as usize];
            view.peek_into(addr::BATTLE_MON_NICK, &mut nick);
            st.active = Some(self.read_battle_mon(view, addr::BATTLE_MON, &nick));

            if st.battle == BattleKind::Trainer {
                let mut tname = [0u8; gen1::NAME_LENGTH as usize];
                view.peek_into(addr::TRAINER_NAME, &mut tname);
                st.trainer_name = text::decode(&tname);
            }
        }

        self.state = st;
    }

    /// Decode one 44-byte `party_struct`.
    fn read_party_mon(&self, view: &DebugView<'_>, base: u16, nick: &[u8]) -> Mon {
        let species = view.peek(base + party::SPECIES);
        let moves: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + party::MOVES + i))
            .collect();
        let pp: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + party::PP + i))
            .collect();
        let stat_exp = super::trainer::derived::StatExp {
            hp: view.peek_be16(base + party::HP_EXP),
            attack: view.peek_be16(base + party::ATTACK_EXP),
            defense: view.peek_be16(base + party::DEFENSE_EXP),
            speed: view.peek_be16(base + party::SPEED_EXP),
            special: view.peek_be16(base + party::SPECIAL_EXP),
        };
        let mut mon = self.finish(
            species,
            nick,
            view.peek(base + party::LEVEL),
            view.peek_be16(base + party::HP),
            view.peek_be16(base + party::MAX_HP),
            Status(view.peek(base + party::STATUS)),
            (
                view.peek(base + party::TYPE1),
                view.peek(base + party::TYPE2),
            ),
            [
                view.peek_be16(base + party::ATTACK),
                view.peek_be16(base + party::DEFENSE),
                view.peek_be16(base + party::SPEED),
                view.peek_be16(base + party::SPECIAL),
            ],
            Dvs::from_bytes(
                view.peek(base + party::DVS),
                view.peek(base + party::DVS + 1),
            ),
            &moves,
            &pp,
        );
        mon.stat_exp = Some(stat_exp);
        mon
    }

    /// Decode one 28-byte `battle_struct` — the same Pokémon, laid out
    /// differently because the battle engine drops the fields it does not need.
    fn read_battle_mon(&self, view: &DebugView<'_>, base: u16, nick: &[u8]) -> Mon {
        let species = view.peek(base + battle::SPECIES);
        let moves: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + battle::MOVES + i))
            .collect();
        let pp: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + battle::PP + i))
            .collect();
        self.finish(
            species,
            nick,
            view.peek(base + battle::LEVEL),
            view.peek_be16(base + battle::HP),
            view.peek_be16(base + battle::MAX_HP),
            Status(view.peek(base + battle::STATUS)),
            (
                view.peek(base + battle::TYPE1),
                view.peek(base + battle::TYPE2),
            ),
            [
                view.peek_be16(base + battle::ATTACK),
                view.peek_be16(base + battle::DEFENSE),
                view.peek_be16(base + battle::SPEED),
                view.peek_be16(base + battle::SPECIAL),
            ],
            Dvs::from_bytes(
                view.peek(base + battle::DVS),
                view.peek(base + battle::DVS + 1),
            ),
            &moves,
            &pp,
        )
    }

    /// Everything both struct forms share: resolving ids to names through the
    /// cartridge's own tables.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        species: u8,
        nick: &[u8],
        level: u8,
        hp: u16,
        max_hp: u16,
        status: Status,
        types: (u8, u8),
        stats: [u16; 4],
        dvs: Dvs,
        moves: &[u8],
        pp: &[u8],
    ) -> Mon {
        Mon {
            species,
            dex: self.tables.dex_number(species),
            name: self.tables.species_name(species),
            nickname: text::decode(nick),
            level,
            hp,
            max_hp,
            status,
            types: (
                self.tables.type_name(types.0),
                self.tables.type_name(types.1),
            ),
            attack: stats[0],
            defense: stats[1],
            speed: stats[2],
            special: stats[3],
            moves: self.tables.move_slots(moves, pp),
            dvs,
            stat_exp: None,
        }
    }
}

impl Plugin for TrainerPlugin {
    fn name(&self) -> &'static str {
        "trainer"
    }

    fn on_attach(&mut self, view: &DebugView<'_>) {
        self.detected_title = view.rom_title();
        self.game = Game::detect(&self.detected_title, view.global_checksum());
    }

    fn on_events(&mut self, events: &[Event], view: &DebugView<'_>) {
        if self.game.is_none() || !self.active {
            return;
        }
        for e in events {
            let Event::Frame { index } = *e;
            if index.saturating_sub(self.last_sample) < SAMPLE_EVERY {
                continue;
            }
            self.last_sample = index;
            self.sample(view);
        }
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

    #[test]
    fn only_the_two_verified_cartridges_are_accepted() {
        assert_eq!(Game::detect("POKEMON BLUE", 0x9D0A), Some(Game::Blue));
        assert_eq!(Game::detect("POKEMON RED", 0x9120), Some(Game::Red));
        // Right title, wrong build: a hack or a translation, whose RAM layout
        // we have not verified.
        assert_eq!(Game::detect("POKEMON BLUE", 0x1234), None);
        // Games this cannot read yet.
        assert_eq!(Game::detect("POKEMON YELLOW", 0x9D0A), None);
        assert_eq!(Game::detect("POKEMON CRYSTAL", 0x9D0A), None);
        assert_eq!(Game::detect("TETRIS", 0x0000), None);
    }

    #[test]
    fn an_unsupported_cartridge_explains_itself() {
        let mut p = TrainerPlugin::new(Vec::new());
        p.detected_title = "TETRIS".to_string();
        let msg = p.refusal().expect("an unknown game must be refused");
        assert!(msg.contains("TETRIS"), "the refusal names the cartridge");
        assert!(msg.contains("Red/Blue"), "and says what it does support");
    }

    #[test]
    fn badge_flags_follow_the_bit_order() {
        let st = TrainerState {
            badges: 0b0000_0101,
            ..Default::default()
        };
        let flags = st.badge_flags();
        assert!(flags[0], "bit 0 is Boulder");
        assert!(!flags[1]);
        assert!(flags[2], "bit 2 is Thunder");
        assert_eq!(st.badge_count(), 2);
        assert_eq!(gen1::BADGE_NAMES[0], "Boulder");
        assert_eq!(gen1::BADGE_NAMES[7], "Earth");
    }

    #[test]
    fn hp_fraction_is_clamped_and_faint_is_zero_hp() {
        let mut m = Mon {
            hp: 12,
            max_hp: 24,
            ..Default::default()
        };
        assert!((m.hp_fraction() - 0.5).abs() < 1e-6);
        m.hp = 0;
        assert!(m.is_fainted());
        assert_eq!(m.hp_fraction(), 0.0);
        // A zero max (an empty slot) must not divide by zero.
        m.max_hp = 0;
        assert_eq!(m.hp_fraction(), 0.0);
    }

    #[test]
    fn a_single_type_mon_is_not_labelled_twice() {
        let m = Mon {
            types: ("NORMAL".into(), "NORMAL".into()),
            ..Default::default()
        };
        assert_eq!(m.type_label(), "NORMAL");
        let m = Mon {
            types: ("GRASS".into(), "POISON".into()),
            ..Default::default()
        };
        assert_eq!(m.type_label(), "GRASS/POISON");
    }
}
