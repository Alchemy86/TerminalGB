//! "There is simply no leaving the damn room" — the Gen 1 Cable Club has no
//! door, and the player has to be told which button gets them out.
//!
//! # What the room really is
//!
//! Red/Blue's two link rooms — the Trade Center and the Colosseum — are the
//! only maps in the game with **no warp events at all**.
//! `data/maps/objects/TradeCenter.asm` writes `def_warp_events` with nothing
//! under it, so `wNumberOfWarps` reads `0` while the player is standing there
//! and there is no tile to walk out through. Measured on the cartridge by
//! `examples/gen1_cancel_exit.rs`.
//!
//! Two things follow, and both surprise people:
//!
//! * **Cancelling the trade screen does not leave the room.**
//!   `.cancelMenuItem_APressed` (`engine/link/cable_club.asm`) sends `$F` and
//!   only falls through when the round comes back `$F` — the disassembly's own
//!   comment on that compare is *"did the other person choose Cancel too?"* —
//!   and what it falls through to is `ReturnToCableClubRoom`, which reloads the
//!   **same** map. So both players must cancel, and even then they are back in
//!   the doorless room.
//! * **The way out is START → RESET.** `StartMenu_SaveReset`
//!   (`engine/menus/start_sub_menus.asm`) jumps to `Init` instead of saving
//!   whenever `BIT_LINK_CONNECTED` is set, which is why the START menu in a
//!   Cable Club room shows RESET where it normally shows SAVE. It is safe by
//!   design: the game saved at the receptionist on the way in, and
//!   `TradeCenter_Trade` saves again the moment a trade completes, with
//!   pokered's own comment *"this allows reset into Pokecenter"*.
//!
//! Generation 2 does not have this problem — its Time Capsule and Trade Corner
//! are ordinary Pokémon Center 2F maps with a staircase — which is exactly why
//! a Gold player watching a Blue player get stuck reads it as the emulator's
//! fault. It is not. Full write-up:
//! [`docs/gen1/leaving-the-cable-club.md`](../../../docs/gen1/leaving-the-cable-club.md).
//!
//! # Why the plugin is the right place for this
//!
//! It is a fact about one pair of cartridges, so it does not belong in the
//! frontend, and it costs one byte at the sample rate the plugin already runs
//! at. The plugin declares no `WRITE_MEMORY`; nothing here writes anything.

/// `wLinkState` — `LINK_STATE_NONE` outside the Cable Club,
/// `LINK_STATE_IN_CABLE_CLUB` (1) while standing in one of its rooms, and
/// `LINK_STATE_TRADING` (`$32`) / `LINK_STATE_START_BATTLE` (3) /
/// `LINK_STATE_BATTLING` (4) inside a session.
///
/// Both places that put the player in a Cable Club room set it — `LinkMenu`
/// (`engine/menus/main_menu.asm`) and `SpecialEnterMap`'s club branch
/// (`engine/link/cable_club.asm`) — so "non-zero" is the whole test.
pub const LINK_STATE: u16 = 0xD12B;

/// `wCurMap`, to tell the two rooms apart.
pub const CUR_MAP: u16 = 0xD35E;

/// `TRADE_CENTER` / `COLOSSEUM` (`constants/map_constants.asm`).
pub const MAP_TRADE_CENTER: u8 = 0xEF;
pub const MAP_COLOSSEUM: u8 = 0xF0;

/// `LINK_STATE_TRADING` (`constants/serial_constants.asm`) — the trade
/// selection screen is up.
pub const LINK_STATE_TRADING: u8 = 0x32;

/// Which link room the player is in, and whether a session is running in it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CableClub {
    /// The Trade Center room, no session on screen.
    TradeCenter,
    /// The Trade Center with the trade selection screen up.
    Trading,
    /// The Colosseum room.
    Colosseum,
    /// A link room this reader does not recognise — still doorless, so still
    /// worth saying, just without naming it.
    Room,
}

impl CableClub {
    /// What the player is looking at, for a status row that has ~18 columns.
    pub fn short(self) -> &'static str {
        match self {
            CableClub::TradeCenter => "Cable Club",
            CableClub::Trading => "Cable Club trade",
            CableClub::Colosseum => "Colosseum",
            CableClub::Room => "Cable Club",
        }
    }

    /// The sentence a player who cannot get out needs, in full.
    ///
    /// It says both halves because they are two different obstacles and fixing
    /// only the first leaves the player exactly where they started.
    pub fn advice(self) -> &'static str {
        match self {
            CableClub::Trading => {
                "Cable Club: BOTH players must choose CANCEL to leave the trade \
                 screen. The room itself has no door — press START and choose \
                 RESET to leave; the game has already saved."
            }
            _ => {
                "Cable Club: this room has no exit — that is the cartridge, not \
                 the emulator. Press START and choose RESET to leave; the game \
                 saved on the way in and again after any trade."
            }
        }
    }
}

/// Read the player's Cable Club situation from two bytes.
///
/// `None` outside the Cable Club, which is every frame of an ordinary game.
pub fn detect(link_state: u8, cur_map: u8) -> Option<CableClub> {
    if link_state == 0 {
        return None;
    }
    Some(match (cur_map, link_state) {
        (MAP_TRADE_CENTER, LINK_STATE_TRADING) => CableClub::Trading,
        (MAP_TRADE_CENTER, _) => CableClub::TradeCenter,
        (MAP_COLOSSEUM, _) => CableClub::Colosseum,
        _ => CableClub::Room,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_game_is_never_in_a_cable_club() {
        // `wLinkState` is `LINK_STATE_NONE` for the whole of a normal game, so
        // whatever map the player is on must produce nothing. Checked over
        // every map id rather than a couple of samples: the Trade Center's id
        // is an ordinary byte and WRAM holds noise before a save loads.
        for map in 0..=0xFFu8 {
            assert_eq!(detect(0, map), None, "map {map:#04x}");
        }
    }

    #[test]
    fn the_two_link_rooms_are_told_apart() {
        assert_eq!(detect(1, MAP_TRADE_CENTER), Some(CableClub::TradeCenter));
        assert_eq!(
            detect(LINK_STATE_TRADING, MAP_TRADE_CENTER),
            Some(CableClub::Trading)
        );
        assert_eq!(detect(1, MAP_COLOSSEUM), Some(CableClub::Colosseum));
        // In the Colosseum `wLinkState` walks 1 -> 3 -> 4; none of those is a
        // trade, and all of them are still a doorless room.
        assert_eq!(detect(4, MAP_COLOSSEUM), Some(CableClub::Colosseum));
    }

    #[test]
    fn every_verdict_names_reset_as_the_way_out() {
        // The advice is the deliverable — a player who is told they are in a
        // Cable Club and not told which button gets them out is no better off.
        for c in [
            CableClub::TradeCenter,
            CableClub::Trading,
            CableClub::Colosseum,
            CableClub::Room,
        ] {
            assert!(c.advice().contains("RESET"), "{c:?}: {}", c.advice());
            assert!(!c.short().is_empty());
        }
        // And the trade screen has a second obstacle in front of the first.
        assert!(CableClub::Trading.advice().contains("BOTH"));
    }
}
