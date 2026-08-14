//! The Generation 1 **link battle**, from the point of view of the other Game
//! Boy — and the addresses that let this partner take part in one honestly.
//!
//! # Why the Colosseum needed its own module
//!
//! `CableClub_DoBattleOrTrade` (`pret/pokered`, `engine/link/cable_club.asm`)
//! is **one function for both Cable Club rooms**. Trade Centre and Colosseum
//! exchange byte-for-byte the same three blocks — random numbers, party data,
//! patch list — and diverge only afterwards, on a value neither console sends:
//! `CableClubLeftGameboy` (`engine/pokemon/bills_pc.asm`) decides from
//! `wCurMap` which room the *player* walked into.
//!
//! So a partner that only knows how to trade will happily complete the
//! Colosseum's party exchange and then answer battle turns with trade answers.
//! It has no way to notice: `LinkBattleExchangeData` (`engine/battle/core.asm`)
//! and `Serial_SyncAndExchangeNybble` (`home/serial.asm`) share one byte
//! encoding, `$60 | n`. That is what this emulator shipped, and it crashed the
//! game — see the conformance record in `docs/link-battle.md`.
//!
//! # The whole vocabulary is four bits
//!
//! The player's console computes everything: damage, accuracy, criticals,
//! status, faints, experience, all of it, from a random list both consoles
//! share. Nothing about a *result* ever crosses the cable. The partner is asked
//! only for an **action**, once per turn:
//!
//! ```text
//! 0-3   use this move slot          core.asm SelectEnemyMove
//! 4+n   send out party slot n       core.asm EnemySendOutFirstMon ("sub 4")
//! $D    LINKBATTLE_NO_ACTION        cannot act this turn
//! $E    LINKBATTLE_STRUGGLE         out of usable moves
//! $F    LINKBATTLE_RUN              leave the battle
//! ```
//!
//! (`constants/serial_constants.asm`.) That is the entire protocol, and none of
//! it is bounds-checked on the receiving side — which is the hazard this module
//! exists to remove.
//!
//! # Reading the host instead of simulating it
//!
//! To answer *legally* the partner must know its own Pokémon's moves, PP and
//! HP. Computing those independently means reimplementing Gen 1's battle engine
//! — damage formula, type chart, criticals, status, the shared RNG stream — and
//! that is both a large piece of work and the classic source of link desyncs
//! between two real cartridges.
//!
//! It is also unnecessary. The player's console is the single authority and it
//! maintains all of it in memory this plugin can already read
//! ([`Capability::READ_MEMORY`](crate::plugin::Capability::READ_MEMORY)), at
//! `wEnemyMon` — "enemy" being *us*, from the host's point of view. So the
//! partner **looks up** the state it needs rather than recomputing it. That is
//! not a faithful two-console emulation and this module does not claim to be
//! one: an observer with a logic analyser could tell. It is a real opponent,
//! choosing real actions, arriving through the genuine link path, with the host
//! doing all the arbitration.
//!
//! # Where these addresses come from
//!
//! The same derivation as [`crate::trainer::gen1`]: `pret/pokered`'s
//! `ram/wram.asm` and `macros/ram.asm`, cross-checked against this cartridge.
//! `wEnemyMons` was additionally **located by measurement, not by faith** — the
//! Colosseum instrument searched WRAM for a `party_struct` matching the active
//! Pokémon's species, level and max HP and found exactly one candidate,
//! `$D8A4`. [`Host::party_hp`] re-checks the species at run time anyway, so a
//! cartridge that disagrees is declined rather than misread.

use crate::plugin::Machine;
use crate::trainer::gen1::addr as trainer_addr;
use crate::trainer::Game;

/// `wLinkState` — the game's own statement of what the link is doing.
///
/// This is the discriminator the whole module turns on, and it is authoritative
/// in a way nothing on the wire is: `cable_club.asm` sets it *before* the first
/// battle turn and the battle engine branches on it every turn
/// (`SelectEnemyMove`). It is also the crash detector — see
/// [`is_valid_link_state`].
pub const LINK_STATE: u16 = 0xD12B;

/// The host's copy of **our** party, six `party_struct` entries of 44 bytes.
/// Species at `+0`, current HP at `+1` big-endian, level at `+33`.
pub const ENEMY_PARTY: u16 = 0xD8A4;

/// Stride of a `party_struct` (`macros/ram.asm`).
pub const PARTY_STRUCT_LEN: u16 = 44;

/// Offsets into `wEnemyMon`, a 29-byte `battle_struct` (`macros/ram.asm`:
/// species, HP `dw`, party position, status, two types, catch rate, four moves,
/// DVs `dw`, level, five stats `dw`, four PP).
pub mod battle_mon {
    pub const HP: u16 = 1;
    pub const PARTY_POS: u16 = 3;
    pub const MOVES: u16 = 8;
    pub const PP: u16 = 25;
}

// `LINK_STATE_*` (`constants/serial_constants.asm`).
pub const LINK_STATE_NONE: u8 = 0x00;
pub const LINK_STATE_IN_CABLE_CLUB: u8 = 0x01;
pub const LINK_STATE_START_TRADE: u8 = 0x02;
pub const LINK_STATE_START_BATTLE: u8 = 0x03;
pub const LINK_STATE_BATTLING: u8 = 0x04;
pub const LINK_STATE_RESET: u8 = 0x05;
pub const LINK_STATE_TRADING: u8 = 0x32;

/// `LINKBATTLE_NO_ACTION` — "my Pokémon cannot choose a move this turn".
pub const LINKBATTLE_NO_ACTION: u8 = 0x0D;
/// `LINKBATTLE_STRUGGLE` — "no usable moves left".
pub const LINKBATTLE_STRUGGLE: u8 = 0x0E;
/// `LINKBATTLE_RUN` — "I am leaving the battle".
pub const LINKBATTLE_RUN: u8 = 0x0F;
/// A replacement answer is `SWITCH_BASE + party slot`.
pub const SWITCH_BASE: u8 = 4;

/// The seven values `wLinkState` can legitimately hold.
///
/// Anything else means the game is no longer running its own code, and it is
/// the single most reliable crash signal available to a Gen 1 plugin — far
/// earlier and far more specific than any CPU-level heuristic. The reproduction
/// this module was written against reaches `$7F` and then `$39`, while the CPU
/// itself never executes an illegal opcode or leaves executable memory.
pub fn is_valid_link_state(v: u8) -> bool {
    matches!(
        v,
        LINK_STATE_NONE
            | LINK_STATE_IN_CABLE_CLUB
            | LINK_STATE_START_TRADE
            | LINK_STATE_START_BATTLE
            | LINK_STATE_BATTLING
            | LINK_STATE_RESET
            | LINK_STATE_TRADING
    )
}

/// Our active Pokémon as the *host* maintains it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct ActiveMon {
    pub species: u8,
    pub hp: u16,
    /// Which of our party slots is out (`wEnemyMonPartyPos`).
    pub party_pos: u8,
    pub moves: [u8; 4],
    pub pp: [u8; 4],
}

/// What the partner may ask the running game about the battle it is in.
///
/// It is a trait rather than a struct of values so that **nothing is read
/// unless it is needed**: a section exchange is 644 transfers and none of them
/// wants any of this. It is also what keeps [`super::protocol::LinkPartner`]
/// pure — the protocol tests drive a fake host and never boot a ROM.
///
/// Every method may answer `None`, which means "this cartridge is not one whose
/// memory map we know". The partner must stay safe in that case; it simply has
/// less to go on.
pub trait Host {
    /// `wLinkState`.
    fn link_state(&mut self) -> Option<u8> {
        None
    }
    /// `wEnemyMon` — our side of the battle.
    fn active(&mut self) -> Option<ActiveMon> {
        None
    }
    /// Current HP of our party slot `slot` in the host's copy of our party.
    /// `None` means it could not be read *or* did not look like the party we
    /// sent, which are the same answer as far as the caller is concerned.
    fn party_hp(&mut self, _slot: u8) -> Option<u16> {
        None
    }
}

/// A host that knows nothing — an unrecognised cartridge, and every unit test.
pub struct NoHost;
impl Host for NoHost {}

/// [`Host`] backed by the real machine, for a cartridge whose RAM map we know.
pub struct MachineHost<'x, 'm, 'a> {
    machine: &'x mut Machine<'m, 'a>,
    /// Species of each slot in the party we put on the wire, used to confirm
    /// that `wEnemyMons` really is the host's copy of it before its HP is
    /// trusted.
    offered_species: &'x [u8],
}

impl<'x, 'm, 'a> MachineHost<'x, 'm, 'a> {
    /// `None` unless this is a cartridge [`crate::trainer::Game::detect`]
    /// recognises — the addresses above are Red/Blue's and mean nothing
    /// elsewhere.
    pub fn new(
        machine: &'x mut Machine<'m, 'a>,
        game: Option<Game>,
        offered_species: &'x [u8],
    ) -> Option<MachineHost<'x, 'm, 'a>> {
        game?;
        Some(MachineHost {
            machine,
            offered_species,
        })
    }
}

impl Host for MachineHost<'_, '_, '_> {
    fn link_state(&mut self) -> Option<u8> {
        self.machine.peek(LINK_STATE)
    }

    fn active(&mut self) -> Option<ActiveMon> {
        let base = trainer_addr::ENEMY_MON;
        let mut moves = [0u8; 4];
        let mut pp = [0u8; 4];
        self.machine.peek_into(base + battle_mon::MOVES, &mut moves);
        self.machine.peek_into(base + battle_mon::PP, &mut pp);
        Some(ActiveMon {
            species: self.machine.peek(base)?,
            // Every HP and stat in Gen 1 is big-endian.
            hp: self.machine.peek_be16(base + battle_mon::HP)?,
            party_pos: self.machine.peek(base + battle_mon::PARTY_POS)?,
            moves,
            pp,
        })
    }

    fn party_hp(&mut self, slot: u8) -> Option<u16> {
        let want = *self.offered_species.get(slot as usize)?;
        let base = ENEMY_PARTY + u16::from(slot) * PARTY_STRUCT_LEN;
        // Refuse to read HP out of a structure that is not the party we sent.
        // This is what makes the located `$D8A4` safe rather than merely
        // probable: a cartridge that lays its WRAM out differently fails here
        // and the partner declines instead of inventing a Pokémon.
        if self.machine.peek(base)? != want {
            return None;
        }
        self.machine.peek_be16(base + 1)
    }
}

/// The action the partner takes this turn, before it is put on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// Use move slot 0-3. **Always a slot that actually holds a move.**
    Move(u8),
    /// Send out party slot n after a faint.
    Switch(u8),
    /// No usable moves.
    Struggle,
    /// Cannot act this turn.
    NoAction,
    /// Leave the battle — the only honest answer when the partner cannot see
    /// which of its Pokémon are still standing.
    Run,
}

impl Action {
    /// The four bits that go on the wire.
    pub fn nybble(self) -> u8 {
        match self {
            Action::Move(slot) => slot & 0x03,
            Action::Switch(slot) => (SWITCH_BASE + slot) & 0x0F,
            Action::Struggle => LINKBATTLE_STRUGGLE,
            Action::NoAction => LINKBATTLE_NO_ACTION,
            Action::Run => LINKBATTLE_RUN,
        }
    }

    pub fn label(self) -> String {
        match self {
            Action::Move(s) => format!("move {}", s + 1),
            Action::Switch(s) => format!("send out #{}", s + 1),
            Action::Struggle => "Struggle".into(),
            Action::NoAction => "no action".into(),
            Action::Run => "run".into(),
        }
    }
}

/// Choose this turn's action.
///
/// `fallback_moves` is the move list of the Pokémon we put on the wire, used
/// when the host cannot be read. It is what keeps the **crash** impossible on
/// any cartridge: the returned slot is always one that holds a move, which is
/// the single thing `SelectEnemyMove` does not check and `GetCurrentMove` then
/// dereferences off the end of the move table.
pub fn choose(host: &mut dyn Host, fallback_moves: [u8; 4], roll: u32) -> Action {
    let active = host.active();

    // A fainted active Pokémon means this rendezvous is `ReplaceFaintedEnemyMon`
    // asking which Pokémon comes next, not `SelectEnemyMove` asking for a move.
    // Answering it with a move index is what turns the opponent into a
    // MissingNo-class glitch: `EnemySendOutFirstMon` does `sub 4` with no bounds
    // check, so move slot 0 becomes party slot 252.
    if active.is_some_and(|a| a.hp == 0) {
        return replacement(host, active.map(|a| a.party_pos));
    }

    let (moves, pp) = match active {
        Some(a) => (a.moves, Some(a.pp)),
        None => (fallback_moves, None),
    };

    // Legal = the slot holds a move, and (when we can see it) that move has PP.
    // A real partner sends Struggle rather than a move it cannot use.
    let usable: Vec<u8> = (0..4u8)
        .filter(|&i| moves[i as usize] != 0)
        .filter(|&i| pp.is_none_or(|p| p[i as usize] > 0))
        .collect();

    if let Some(&slot) = usable.get(roll as usize % usable.len().max(1)) {
        return Action::Move(slot);
    }
    if moves.iter().any(|&m| m != 0) {
        // Moves exist but none has PP left.
        Action::Struggle
    } else {
        // No moves at all. Not reachable from a well-formed party, but the wire
        // must still carry something the receiver bounds-checks.
        Action::NoAction
    }
}

/// Which Pokémon to send out after ours faints.
fn replacement(host: &mut dyn Host, current: Option<u8>) -> Action {
    for slot in 0..6u8 {
        if Some(slot) == current {
            continue;
        }
        match host.party_hp(slot) {
            Some(hp) if hp > 0 => return Action::Switch(slot),
            Some(_) => continue,
            // The host's party is unreadable, so any slot we named would be a
            // guess — and a wrong guess sends out a fainted Pokémon or indexes
            // past the party entirely. Running is a defined move in this
            // protocol (`ReplaceFaintedEnemyMon` returns straight to
            // `EnemyRan`), so the battle ends cleanly instead.
            None => return Action::Run,
        }
    }
    Action::Run
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A host with a scripted answer, so the chooser can be tested without a ROM.
    #[derive(Default)]
    struct Fake {
        link_state: Option<u8>,
        active: Option<ActiveMon>,
        party: Vec<Option<u16>>,
    }
    impl Host for Fake {
        fn link_state(&mut self) -> Option<u8> {
            self.link_state
        }
        fn active(&mut self) -> Option<ActiveMon> {
            self.active
        }
        fn party_hp(&mut self, slot: u8) -> Option<u16> {
            self.party.get(slot as usize).copied().flatten()
        }
    }

    fn active(hp: u16, moves: [u8; 4], pp: [u8; 4]) -> Option<ActiveMon> {
        Some(ActiveMon {
            species: 0x28,
            hp,
            party_pos: 0,
            moves,
            pp,
        })
    }

    /// The crash. `GetCurrentMove` does `dec a; AddNTimes Moves, 6` on the move
    /// id it is handed, so move id 0 indexes 1,530 bytes past a 165-entry table
    /// and `_JumpMoveEffect` `jp hl`s into whatever it finds. The one thing this
    /// partner must never do is name an empty slot.
    #[test]
    fn a_move_is_never_chosen_from_an_empty_slot() {
        for moves in [
            [1, 0, 0, 0],
            [1, 2, 0, 0],
            [1, 2, 3, 0],
            [1, 2, 3, 4],
            [0, 0, 0, 0],
        ] {
            for roll in 0..16u32 {
                // With no host view at all — the floor that holds on any
                // cartridge.
                let a = choose(&mut NoHost, moves, roll);
                if let Action::Move(slot) = a {
                    assert_ne!(
                        moves[slot as usize], 0,
                        "chose empty slot {slot} of {moves:?}"
                    );
                }
                // ...and with one.
                let mut h = Fake {
                    active: active(100, moves, [10; 4]),
                    ..Fake::default()
                };
                if let Action::Move(slot) = choose(&mut h, [0; 4], roll) {
                    assert_ne!(moves[slot as usize], 0);
                }
            }
        }
    }

    /// A partner with no moves left sends Struggle, as a second console does.
    /// Sending a 0-PP move index instead makes the host underflow its own PP.
    #[test]
    fn no_pp_left_is_struggle_not_a_move() {
        let mut h = Fake {
            active: active(100, [1, 2, 0, 0], [0, 0, 0, 0]),
            ..Fake::default()
        };
        assert_eq!(choose(&mut h, [1, 2, 0, 0], 0), Action::Struggle);
    }

    #[test]
    fn a_move_with_pp_is_preferred_over_one_without() {
        let mut h = Fake {
            active: active(100, [1, 2, 3, 4], [0, 0, 7, 0]),
            ..Fake::default()
        };
        for roll in 0..8 {
            assert_eq!(choose(&mut h, [0; 4], roll), Action::Move(2));
        }
    }

    /// After a faint the rendezvous is `ReplaceFaintedEnemyMon`, which wants
    /// `4 + slot`. Answering with a move index makes `wWhichPokemon` underflow
    /// to party slot 252 and the game fields a glitch Pokémon.
    #[test]
    fn a_fainted_active_is_answered_with_a_replacement() {
        let mut h = Fake {
            active: active(0, [1, 0, 0, 0], [10, 0, 0, 0]),
            party: vec![Some(0), Some(31)],
            ..Fake::default()
        };
        let a = choose(&mut h, [1, 0, 0, 0], 0);
        assert_eq!(a, Action::Switch(1));
        assert_eq!(a.nybble(), 5, "the wire carries 4 + slot");
    }

    /// ...and when the host's party cannot be read, the partner leaves rather
    /// than naming a slot it cannot see.
    #[test]
    fn an_unreadable_party_forfeits_instead_of_guessing() {
        let mut h = Fake {
            active: active(0, [1, 0, 0, 0], [10, 0, 0, 0]),
            party: vec![],
            ..Fake::default()
        };
        assert_eq!(choose(&mut h, [1, 0, 0, 0], 0), Action::Run);
        assert_eq!(Action::Run.nybble(), LINKBATTLE_RUN);
    }

    #[test]
    fn every_action_fits_in_a_nybble() {
        for a in [
            Action::Move(3),
            Action::Switch(5),
            Action::Struggle,
            Action::NoAction,
            Action::Run,
        ] {
            assert!(a.nybble() <= 0x0F, "{a:?}");
        }
    }

    /// The crash signal. `$7F` and `$39` are the values the real reproduction
    /// reaches; every legitimate value must be accepted.
    #[test]
    fn only_the_seven_real_link_states_are_valid() {
        for v in [0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x32] {
            assert!(is_valid_link_state(v), "{v:#04x}");
        }
        for v in [0x06, 0x10, 0x39, 0x7F, 0xFF] {
            assert!(!is_valid_link_state(v), "{v:#04x}");
        }
    }

    /// The battle struct is 29 bytes and these offsets are load-bearing: read
    /// PP one byte out and the partner sends Struggle at full PP.
    #[test]
    fn the_battle_struct_offsets_match_the_documented_addresses() {
        let base = trainer_addr::ENEMY_MON;
        assert_eq!(base, 0xCFE5, "wEnemyMon");
        assert_eq!(base + battle_mon::MOVES, 0xCFED, "wEnemyMonMoves");
        assert_eq!(base + battle_mon::PP, 0xCFFE, "wEnemyMonPP");
        assert_eq!(base + battle_mon::PARTY_POS, 0xCFE8, "wEnemyMonPartyPos");
    }
}
