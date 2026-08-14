//! [`PortalSource`] — the network implementation of the [`PokemonSource`] seam.
//!
//! The fetch happens **eagerly, at construction** — the same philosophy as
//! `TradeSpec::load` building the party blocks up front: a network failure, a
//! signed-out session or a malformed spec is reported at attach time with a
//! message the user can act on, never discovered mid-trade with a Pokémon
//! already on the table. After construction this source touches the network
//! not at all; the trade itself runs entirely against the local partner.

use super::client::{PendingTrade, Portal};
use super::PortalError;
use terminalgb_plugin_gen1_link::spec::{PokemonSource, SourceError, TradeSpec};

/// A pending portal trade, ready to arm the gen1-link partner.
pub struct PortalSource {
    /// The trade id, for `POST /api/trades/<id>/done` when the trade lands.
    pub trade_id: String,
    /// The friend on the far side, for display.
    pub friend: String,
    /// What this player agreed to give, as display text.
    pub give: String,
    spec: TradeSpec,
}

impl PortalSource {
    /// Fetch this player's pending trade. `Ok(None)` means the portal is
    /// reachable and there is simply nothing waiting — a normal state.
    /// If several trades are pending, the first (oldest) is armed and the
    /// count is surfaced in the description.
    pub fn fetch(portal: &Portal) -> Result<Option<PortalSource>, PortalError> {
        let mut trades = portal.trades_mine()?;
        if trades.is_empty() {
            return Ok(None);
        }
        let waiting = trades.len();
        let t = trades.remove(0);
        Ok(Some(PortalSource::from_trade(t, waiting, portal.base())))
    }

    fn from_trade(t: PendingTrade, waiting: usize, portal: &str) -> PortalSource {
        let origin = if waiting > 1 {
            format!(
                "portal trade #{} from {} ({} waiting) — {portal}",
                t.id, t.friend, waiting
            )
        } else {
            format!("portal trade #{} from {} — {portal}", t.id, t.friend)
        };
        PortalSource {
            spec: TradeSpec {
                trainer: t.trainer,
                mons: vec![t.receive],
                origin,
            },
            trade_id: t.id,
            friend: t.friend,
            give: t.give,
        }
    }

    /// The species the friend is sending, for toasts.
    pub fn incoming(&self) -> &str {
        &self.spec.mons[0].species
    }

    /// Inherent copy of [`PokemonSource::describe`], for callers that do not
    /// import the trait. The test harness in particular must not link the
    /// plugin crate directly: with `[profile.release] panic = 'abort'` on the
    /// lib targets, a test target naming the plugin crate can resolve it to a
    /// *second* rlib (unwind-built) and the trait then looks unimplemented —
    /// the same two-variant trap `AGENTS.md` records for `cargo test`.
    pub fn describe_line(&self) -> String {
        use terminalgb_plugin_gen1_link::spec::PokemonSource as _;
        self.describe()
    }
}

impl PokemonSource for PortalSource {
    fn describe(&self) -> String {
        self.spec.describe()
    }

    /// Resolution runs against the receiving player's own cartridge tables,
    /// exactly like a trade file — which is what makes a Pokémon that cannot
    /// exist on this cartridge unarmable rather than merely inadvisable.
    fn party(
        &self,
        tables: &terminalgb_plugin_gen1_link::romdata::RomTables,
    ) -> Result<terminalgb_plugin_gen1_link::party::Party, SourceError> {
        self.spec.party(tables)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_gen1_link::spec::MonSpec;

    fn a_trade() -> PendingTrade {
        PendingTrade {
            id: "71".into(),
            friend: "ben".into(),
            give: "CHARIZARD lv36".into(),
            trainer: "BEN".into(),
            receive: MonSpec {
                species: "Bulbasaur".into(),
                level: 14,
                ..MonSpec::default()
            },
        }
    }

    #[test]
    fn the_description_names_the_trade_the_friend_and_the_portal() {
        let s = PortalSource::from_trade(a_trade(), 1, "https://p.example");
        let d = s.describe();
        assert!(d.contains("Bulbasaur"), "{d}");
        assert!(d.contains("#71"), "{d}");
        assert!(d.contains("ben"), "{d}");
        assert!(d.contains("https://p.example"), "{d}");
    }

    #[test]
    fn extra_waiting_trades_are_not_hidden() {
        let s = PortalSource::from_trade(a_trade(), 3, "p");
        assert!(s.describe().contains("3 waiting"), "{}", s.describe());
    }

    #[test]
    fn the_partner_wears_the_friends_trainer_name() {
        let s = PortalSource::from_trade(a_trade(), 1, "p");
        assert_eq!(s.spec.trainer, "BEN");
        assert_eq!(s.incoming(), "Bulbasaur");
    }
}
