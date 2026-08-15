//! A live trainer view: the party, the badges and the battle opponent, read out
//! of a running Gen 1 Pokémon game.
//!
//! This is a consumer of the plugin surface in [`terminalgb_plugin_api`], and it is
//! deliberately **read-only**: it declares
//! [`Capability::OBSERVE`](terminalgb_plugin_api::Capability::OBSERVE) and
//! [`READ_MEMORY`](terminalgb_plugin_api::Capability::READ_MEMORY) and nothing else, so
//! [`Machine::poke`](terminalgb_plugin_api::Machine::poke) refuses it outright. A
//! tracker that can corrupt a save is worse than no tracker, and that is
//! enforced by the host rather than promised in a comment.
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

pub mod cableclub;
pub mod capture;
pub mod collection;
pub mod derived;
pub mod encounter;
pub mod gen1;
pub mod rom;
pub mod sprite;
pub mod text;

use std::collections::HashMap;

use cableclub::CableClub;
use encounter::WildEncounter;
use gen1::{addr, battle, party, BattleKind, BattleType, Dvs, Mon, Status};
use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};

/// How often the party is re-read, in emulated frames. Six frames is about
/// 10 Hz — a tenth of a second behind the game, which no one can see, for a
/// tenth of the work.
pub const SAMPLE_EVERY: u64 = 6;

/// The name this plugin registers under, in `plugins = …` and the status bar.
pub const NAME: &str = "trainer";

/// Reads through a capability-checked [`Machine`], with the plugin's own
/// "unreadable is `0xFF`" convention applied once here.
///
/// The panel would rather show open-bus values than carry an `Option` through
/// every field: a refused read means the plugin was misconfigured (it declared
/// no `READ_MEMORY`), which the status line reports, and is not a per-address
/// condition worth branching on sixty times a sample.
struct Reads<'x, 'm>(&'x mut Machine<'m>);

impl Reads<'_, '_> {
    fn peek(&mut self, a: u16) -> u8 {
        self.0.peek(a).unwrap_or(0xFF)
    }

    fn peek_be16(&mut self, a: u16) -> u16 {
        self.0.peek_be16(a).unwrap_or(0)
    }

    fn peek_into(&mut self, base: u16, out: &mut [u8]) {
        self.0.peek_into(base, out);
    }
}

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
    /// Which flavour of wild battle, when [`TrainerState::battle`] is
    /// [`BattleKind::Wild`]. Meaningless otherwise.
    pub battle_type: BattleType,
    /// The opposing Pokémon, present only during a battle.
    pub opponent: Option<Mon>,
    /// The player's active Pokémon as the battle engine sees it — its stats
    /// here include in-battle modifiers, which the party copy does not.
    pub active: Option<Mon>,
    /// The opposing trainer's name, in a trainer battle.
    pub trainer_name: String,
    /// The opposing trainer's whole roster, in a trainer battle — the thing an
    /// Ironmon run wants most after the active opponent's stats: how many are
    /// left and what is coming next. Empty for a wild encounter, and empty
    /// while the enemy block has not yet passed its consistency check.
    pub enemy_team: Vec<TeamMon>,
    /// `wEnemyMonActualCatchRate` during a **wild** encounter, and `None`
    /// otherwise: a trainer's Pokémon cannot be caught, so a catch chance there
    /// is noise at best and a lie at worst. See [`capture`].
    pub catch_rate: Option<u8>,
    /// How the best available ball's chance has moved since the last sample,
    /// so the panel can show which way it is going. `None` on the first sample
    /// of an encounter, when there is nothing to compare against.
    pub catch_delta: Option<f64>,
    /// True when a trainer battle is on but the roster did not read back
    /// consistently — so the panel can say "unknown" instead of showing
    /// nothing, which reads as "no team".
    pub enemy_team_unknown: bool,
}

/// One member of the opposing trainer's roster, as the team strip shows it.
///
/// Deliberately much less than a [`Mon`]: the strip is a picture, a name and
/// whether it is still standing. The *active* opponent gets the full treatment
/// through [`TrainerState::opponent`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TeamMon {
    /// Internal index number, 0 when the slot is not readable.
    pub species: u8,
    pub name: String,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    /// False when this slot's data did not read back consistently — shown as
    /// `?` rather than guessed at.
    pub known: bool,
}

impl TeamMon {
    pub fn is_fainted(&self) -> bool {
        self.known && self.max_hp > 0 && self.hp == 0
    }
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

    /// Whether this is a wild encounter whose catch chance is worth showing.
    pub fn catchable(&self) -> bool {
        self.catch_rate.is_some()
            && self.battle == BattleKind::Wild
            && self.opponent.is_some()
    }

    /// The odds for every ball worth comparing, best last — an empty vector
    /// when this is not a catchable encounter.
    pub fn catch_odds(&self) -> Vec<capture::Odds> {
        let (Some(rate), Some(o)) = (self.catch_rate, self.opponent.as_ref()) else {
            return Vec::new();
        };
        if self.battle != BattleKind::None && self.battle != BattleKind::Wild {
            return Vec::new();
        }
        let t = capture::Target::of(o, rate);
        capture::ball_set(self.battle_type == gen1::BattleType::Safari)
            .iter()
            .map(|&b| capture::odds(t, b))
            .collect()
    }

    /// The chance the best of those balls has, which is what the direction
    /// marker tracks.
    pub fn best_catch_chance(&self) -> Option<f64> {
        self.catch_odds()
            .into_iter()
            .map(|o| o.chance)
            .fold(None, |acc: Option<f64>, c| {
                Some(acc.map_or(c, |a| a.max(c)))
            })
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
    /// plugin still runs [`TrainerPlugin::watch_encounter`] — one byte at
    /// ~10 Hz — and nothing else.
    active: bool,
    last_sample: u64,
    /// The current wild encounter's verdict, latched. See [`encounter`].
    encounter: Option<WildEncounter>,
    /// Bumped every time a *new* encounter is latched, so a frontend can tell
    /// "a Pokémon just appeared" from "the same one is still there" without
    /// cloning or comparing the encounter itself.
    encounter_seq: u64,
    /// Set once by [`TrainerPlugin::on_attach`] so the frontend can explain the
    /// refusal instead of showing an empty panel.
    detected_title: String,
    /// The best ball's chance at the previous sample, so the panel can show the
    /// direction of travel. Cleared the moment an encounter ends, so a stale
    /// arrow can never survive into the next one.
    last_catch: Option<f64>,
    /// Which doorless link room the player is standing in, if any. Watched with
    /// the panel closed for the same reason the encounter is: being stuck is
    /// exactly when nobody has a stats panel up. See [`cableclub`].
    cable_club: Option<CableClub>,
    /// Bumped when the player *enters* a link room, so a frontend can say the
    /// sentence once instead of every frame.
    cable_club_seq: u64,
}

impl TrainerPlugin {
    /// Build a plugin against a cartridge image.
    ///
    /// The ROM is passed in whole rather than read through
    /// [`Machine::peek_bank`](terminalgb_plugin_api::Machine::peek_bank) because
    /// [`rom::RomTables`] *scans every bank* to locate its tables by signature,
    /// which is a different need from reading a known table out of a known bank
    /// — that is what `peek_bank` is for, and it is what closed the banked-read
    /// gap this plugin originally reported.
    pub fn new(rom: Vec<u8>) -> TrainerPlugin {
        TrainerPlugin {
            game: None,
            tables: rom::RomTables::new(rom),
            state: TrainerState::default(),
            sprites: HashMap::new(),
            active: false,
            last_sample: 0,
            encounter: None,
            encounter_seq: 0,
            detected_title: String::new(),
            last_catch: None,
            cable_club: None,
            cable_club_seq: 0,
        }
    }

    /// The game, or `None` when the cartridge is not one this can read.
    pub fn game(&self) -> Option<Game> {
        self.game
    }

    /// The cartridge's own tables, for callers that decode game data outside
    /// the sampled panel state — [`collection::decode`] most of all, which a
    /// frontend runs *after* its `with_machine` borrow ends.
    pub fn tables(&self) -> &rom::RomTables {
        &self.tables
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

    /// The wild encounter currently on screen, if the game is in one a Ball can
    /// be spent on. `None` in the overworld, in a trainer battle, and during the
    /// old man's catching demonstration — see [`encounter::applies`].
    ///
    /// **Live whether or not the panel is open.** An encounter is exactly the
    /// moment nobody is looking at a stats panel, so the watch is not gated on
    /// [`TrainerPlugin::set_active`].
    pub fn encounter(&self) -> Option<&WildEncounter> {
        self.encounter.as_ref()
    }

    /// Counts encounters latched since the session started. A frontend that
    /// remembers the last value it saw can raise a one-shot alert (a toast, a
    /// bell) exactly once per Pokémon.
    pub fn encounter_seq(&self) -> u64 {
        self.encounter_seq
    }

    /// Pretend the cartridge was recognised, so [`TrainerPlugin::status`] can be
    /// checked without one. Not compiled outside the crate's own unit tests.
    #[cfg(test)]
    pub fn set_game_for_test(&mut self, game: Option<Game>) {
        self.game = game;
    }

    /// Plant an encounter without a cartridge, so the frontend's layout and
    /// alert behaviour can be tested against a synthetic ROM.
    ///
    /// Public rather than `#[cfg(test)]` because the frontend that renders this
    /// panel is now a different crate, and a `cfg(test)` item is invisible
    /// across a crate boundary. That is the ordinary cost of a real boundary:
    /// a test hook another crate needs is part of the surface, and saying so
    /// is better than the alternatives (a `testing` feature for one method, or
    /// deleting the frontend's test).
    /// Plant a Cable Club room without a cartridge, for the same reason and on
    /// the same terms as [`TrainerPlugin::set_encounter_for_test`].
    pub fn set_cable_club_for_test(&mut self, room: Option<CableClub>) {
        if room.is_some() && self.cable_club.is_none() {
            self.cable_club_seq += 1;
        }
        self.cable_club = room;
    }

    pub fn set_encounter_for_test(&mut self, enc: Option<WildEncounter>) {
        if enc.is_some() {
            self.encounter_seq += 1;
        }
        self.encounter = enc;
    }

    /// The whole per-frame cost when the panel is closed: read `wIsInBattle`,
    /// and in a wild battle whose verdict is not yet latched, read enough of
    /// `wEnemyMon` to latch it.
    ///
    /// Ordinary case, in the overworld: **one byte, six times a second.**
    fn watch_encounter(&mut self, machine: &mut Machine<'_>) {
        let view = &mut Reads(machine);
        let in_battle = view.peek(addr::IS_IN_BATTLE);
        // The second byte is only read once the first has said "wild battle",
        // so the overworld really does cost one peek.
        let battle_type = if BattleKind::from_flag(in_battle) == BattleKind::Wild {
            view.peek(addr::BATTLE_TYPE)
        } else {
            0
        };
        if !encounter::applies(in_battle, battle_type) {
            self.encounter = None;
            return;
        }
        // Already decided for this battle: the DVs cannot change under us, and
        // holding the verdict is what makes it persist for the encounter.
        if self.encounter.is_some() {
            return;
        }
        let species = view.peek(addr::ENEMY_MON + battle::SPECIES);
        let dex = self.tables.dex_number(species);
        let level = view.peek(addr::ENEMY_MON + battle::LEVEL);
        let hp = view.peek_be16(addr::ENEMY_MON + battle::HP);
        let max_hp = view.peek_be16(addr::ENEMY_MON + battle::MAX_HP);
        if !encounter::looks_populated(dex, level, hp, max_hp) {
            return;
        }
        self.encounter = Some(WildEncounter {
            species,
            dex,
            name: self.tables.species_name(species),
            level,
            dvs: Dvs::from_bytes(
                view.peek(addr::ENEMY_MON + battle::DVS),
                view.peek(addr::ENEMY_MON + battle::DVS + 1),
            ),
            kind: BattleType::from_flag(battle_type),
        });
        self.encounter_seq += 1;
    }

    /// Which doorless Cable Club room the player is standing in, if any.
    ///
    /// **Live whether or not the panel is open** — see [`cableclub`] for why
    /// the answer is worth volunteering at all.
    pub fn cable_club(&self) -> Option<CableClub> {
        self.cable_club
    }

    /// Bumped each time the player *enters* a link room, so a frontend can tell
    /// "just walked in" from "still in there" and say its piece once.
    pub fn cable_club_seq(&self) -> u64 {
        self.cable_club_seq
    }

    /// Two bytes: is the player in a Cable Club room, and which one.
    fn watch_cable_club(&mut self, machine: &mut Machine<'_>) {
        let view = &mut Reads(machine);
        let link_state = view.peek(cableclub::LINK_STATE);
        // The map is only read once `wLinkState` has said "in a Cable Club",
        // so an ordinary game costs one peek.
        let now = if link_state == 0 {
            None
        } else {
            cableclub::detect(link_state, view.peek(cableclub::CUR_MAP))
        };
        if now.is_some() && self.cable_club.is_none() {
            self.cable_club_seq += 1;
        }
        self.cable_club = now;
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
    fn sample(&mut self, machine: &mut Machine<'_>) {
        let view = &mut Reads(machine);
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
        // Read before the battle branch so it is cleared on every sample that
        // is not a live wild encounter.
        let mut catch_rate = None;
        st.battle_type = BattleType::from_flag(view.peek(addr::BATTLE_TYPE));
        if st.battle != BattleKind::None {
            let mut nick = [0u8; gen1::NAME_LENGTH as usize];
            view.peek_into(addr::ENEMY_MON_NICK, &mut nick);
            st.opponent = Some(self.read_battle_mon(
                view,
                addr::ENEMY_MON,
                addr::ENEMY_STAT_MODS,
                &nick,
            ));

            let mut nick = [0u8; gen1::NAME_LENGTH as usize];
            view.peek_into(addr::BATTLE_MON_NICK, &mut nick);
            st.active = Some(self.read_battle_mon(
                view,
                addr::BATTLE_MON,
                addr::PLAYER_STAT_MODS,
                &nick,
            ));

            if st.battle == BattleKind::Wild {
                catch_rate = Some(view.peek(addr::ENEMY_ACTUAL_CATCH_RATE));
            }
            if st.battle == BattleKind::Trainer {
                let mut tname = [0u8; gen1::NAME_LENGTH as usize];
                view.peek_into(addr::TRAINER_NAME, &mut tname);
                st.trainer_name = text::decode(&tname);
                self.read_enemy_team(view, &mut st);
            }
        }

        st.catch_rate = catch_rate;
        // The direction of travel, which is the half of this the captain asked
        // for: a number that moves is only useful if you can see it move.
        let now = st
            .best_catch_chance()
            .filter(|_| st.battle == BattleKind::Wild);
        st.catch_delta = match (self.last_catch, now) {
            (Some(prev), Some(c)) => Some(c - prev),
            _ => None,
        };
        self.last_catch = now;
        self.state = st;
    }

    /// Read the opposing trainer's roster — and **refuse it unless it is
    /// self-consistent**.
    ///
    /// The enemy block is documented in [`gen1::addr`] as mirroring the
    /// player's, but an address is only a claim until something checks it, and
    /// the cost of being wrong here is a panel confidently showing a team that
    /// is not there. So three things must hold before any of it is displayed:
    /// the count is in range; every listed species is non-zero; and the species
    /// list agrees with the species field of the `party_struct` beside it —
    /// two independent copies of the same fact, written by the game at
    /// different times. A slot that fails is marked `known: false` and drawn as
    /// `?`; a block that fails wholesale sets
    /// [`TrainerState::enemy_team_unknown`].
    ///
    /// This also covers the real transient: the first frames of a trainer
    /// battle, before `LoadEnemyMonData` has run, where the block still holds
    /// the previous battle's contents.
    fn read_enemy_team(&self, view: &mut Reads<'_, '_>, st: &mut TrainerState) {
        let count = view.peek(addr::ENEMY_PARTY_COUNT) as usize;
        if count == 0 || count > gen1::PARTY_LENGTH {
            st.enemy_team_unknown = true;
            return;
        }
        let mut team = Vec::with_capacity(count);
        let mut any_known = false;
        for i in 0..count as u16 {
            let listed = view.peek(addr::ENEMY_PARTY_SPECIES + i);
            let base = addr::ENEMY_MONS + i * gen1::PARTY_MON_SIZE;
            let stored = view.peek(base + party::SPECIES);
            if listed == 0 || listed != stored {
                team.push(TeamMon::default());
                continue;
            }
            any_known = true;
            team.push(TeamMon {
                species: listed,
                name: self.tables.species_name(listed),
                level: view.peek(base + party::LEVEL),
                hp: view.peek_be16(base + party::HP),
                max_hp: view.peek_be16(base + party::MAX_HP),
                known: true,
            });
        }
        if !any_known {
            st.enemy_team_unknown = true;
            return;
        }
        st.enemy_team = team;
    }

    /// Decode one 44-byte `party_struct`.
    fn read_party_mon(&self, view: &mut Reads<'_, '_>, base: u16, nick: &[u8]) -> Mon {
        let species = view.peek(base + party::SPECIES);
        let moves: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + party::MOVES + i))
            .collect();
        let pp: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + party::PP + i))
            .collect();
        let stat_exp = crate::derived::StatExp {
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
    fn read_battle_mon(
        &self,
        view: &mut Reads<'_, '_>,
        base: u16,
        mods: u16,
        nick: &[u8],
    ) -> Mon {
        let species = view.peek(base + battle::SPECIES);
        let moves: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + battle::MOVES + i))
            .collect();
        let pp: Vec<u8> = (0..gen1::NUM_MOVES as u16)
            .map(|i| view.peek(base + battle::PP + i))
            .collect();
        let mut raw = [0u8; 6];
        view.peek_into(mods, &mut raw);
        let mut mon = self.finish(
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
        );
        mon.stat_mods = Some(crate::derived::StatMods::from_bytes(raw));
        mon
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
            stat_mods: None,
        }
    }
}

impl Plugin for TrainerPlugin {
    fn name(&self) -> &str {
        NAME
    }

    /// Read-only, and declared as such. It never asks for
    /// [`Capability::WRITE_MEMORY`], so [`Machine::poke`] refuses it — a tracker
    /// that can corrupt a save is worse than no tracker, and here that is
    /// enforced by the host rather than promised by the author.
    fn capabilities(&self) -> Capability {
        Capability::OBSERVE | Capability::READ_MEMORY
    }

    fn status(&self) -> String {
        // A player who cannot get out of the room outranks everything else the
        // row could say — and unlike the encounter verdict it is *actionable*.
        if let (Some(_), Some(c)) = (self.game, self.cable_club) {
            return c.short().to_string();
        }
        match (self.game, self.encounter.as_ref()) {
            (None, _) => "unsupported cartridge".to_string(),
            // A live encounter outranks the party count: it is the thing the
            // status line exists to make un-missable.
            (Some(_), Some(e)) if e.would_be_shiny() => {
                format!("★ {} would be SHINY if traded", e.name)
            }
            (Some(_), Some(e)) => format!("wild {} DV {}", e.name, e.dv_string()),
            (Some(_), None) if !self.active => "idle".to_string(),
            (Some(_), None) => format!("{} in party", self.state.party.len()),
        }
    }

    fn on_attach(&mut self, machine: &mut Machine) {
        self.detected_title = machine.rom_title();
        self.game = Game::detect(&self.detected_title, machine.global_checksum());
    }

    fn on_event(&mut self, event: &Event, machine: &mut Machine) {
        if self.game.is_none() {
            return;
        }
        let Event::Frame { number } = *event else {
            return;
        };
        if number.saturating_sub(self.last_sample) < SAMPLE_EVERY {
            return;
        }
        self.last_sample = number;
        // The encounter watch runs whether or not the panel is open: a wild
        // Pokémon appearing is exactly the moment nobody has a stats panel up,
        // and it costs one byte at ~10 Hz. The full party read stays gated on
        // the panel being visible.
        self.watch_encounter(machine);
        self.watch_cable_club(machine);
        if self.active {
            self.sample(machine);
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

    /// The Status panel's plugin row is a *second* place the verdict shows up
    /// with the trainer panel closed, so it is held to the same wording rule as
    /// the banner: never bare "shiny".
    #[test]
    fn the_plugin_status_line_carries_the_verdict() {
        use encounter::WildEncounter;
        use gen1::Dvs;
        let mut p = TrainerPlugin::new(Vec::new());
        p.set_game_for_test(Some(Game::Blue));
        let mut enc = WildEncounter {
            species: 0x24,
            dex: 16,
            name: "PIDGEY".into(),
            level: 3,
            dvs: Dvs::from_bytes(0xEA, 0xAA), // the canonical shiny pair
            kind: BattleType::Normal,
        };
        p.set_encounter_for_test(Some(enc.clone()));
        let s = p.status();
        assert!(s.contains('★') && s.contains("SHINY"), "{s:?}");
        assert!(s.contains("if traded"), "never bare shiny: {s:?}");
        assert!(s.contains("PIDGEY"), "{s:?}");

        // An ordinary encounter still names the Pokémon and its DVs, and never
        // uses the word at all.
        enc.dvs = Dvs::from_bytes(0x9A, 0x88);
        p.set_encounter_for_test(Some(enc));
        let s = p.status();
        assert!(!s.to_uppercase().contains("SHINY"), "{s:?}");
        assert!(s.contains("wild PIDGEY") && s.contains("DV "), "{s:?}");

        // With no encounter it falls back to the panel-closed state.
        p.set_encounter_for_test(None);
        assert_eq!(p.status(), "idle");
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
