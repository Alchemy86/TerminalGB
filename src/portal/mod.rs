//! The trade-portal client — the emulator's half of asynchronous friend trades.
//!
//! The portal (a separate web app, built in its own repository) stores *offers*
//! and matches them into *trades*. Nothing here synchronises two consoles: each
//! player's link cable is answered by the local [`gen1-link`] partner, and what
//! this module contributes is **where that partner's Pokémon comes from** — a
//! network implementation of the [`PokemonSource`] seam that `spec.rs` designed
//! for exactly this ("a remote player … [is an] additional implementation of
//! this one trait, not changes to the protocol").
//!
//! What crosses the wire is a *specification* — species name, level, nickname,
//! OT, moves, DVs; ~250 bytes of JSON — resolved against the receiving player's
//! **own cartridge tables** like every other source. No save, no ROM and no
//! game bytes are ever uploaded, and a malformed Pokémon cannot arrive: a
//! species or move that does not resolve is refused before the partner is
//! armed, by the same [`TradeSpec`] validation the trade-file path uses.
//!
//! # Layers
//!
//! * [`http`] — a minimal HTTPS client that **delegates to the system `curl`
//!   binary**. Deliberate: this repo treats its dependency count as a contract
//!   (`make embed`), and a TLS stack is the one thing that must never be
//!   hand-rolled, so the transport comes from the operating system instead of
//!   a crate. The token travels on `curl`'s stdin, never on argv.
//! * [`auth`] — RFC 8628 device-code sign-in and the on-disk token, stored
//!   under the XDG state directory with mode `0600`.
//! * [`login`] — the two front ends of that flow: a blocking one for
//!   `terminal login`, and a thread-backed [`login::LoginFlow`] the TUI polls
//!   so signing in never stalls a frame.
//! * [`client`] — the HTTP contract: offers, a friend's offers, accept,
//!   `trades/mine`, done. The full request/response shapes are documented in
//!   `docs/portal.md`; the portal repo's `docs/api-contract.md` is the shared
//!   agreement.
//! * [`source`] — [`source::PortalSource`], the `PokemonSource` itself.
//! * [`browser`] — opening the verification page, fail-soft: the URL and code
//!   are always printed too, so a headless session still signs in.
//!
//! [`gen1-link`]: terminalgb_plugin_gen1_link
//! [`PokemonSource`]: terminalgb_plugin_gen1_link::spec::PokemonSource
//! [`TradeSpec`]: terminalgb_plugin_gen1_link::spec::TradeSpec

pub mod auth;
pub mod browser;
pub mod card;
pub mod client;
pub mod http;
pub mod login;
pub mod offer;
pub mod source;

/// One error type for the whole module, so every caller can print something a
/// user can act on. Failures here gate a trade, and a trade is irreversible —
/// nothing in this module is allowed to fail quietly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortalError {
    /// The transport failed (curl missing, DNS, refused, timeout).
    Http(String),
    /// The portal answered, but not with what the contract promises.
    Contract(String),
    /// Signed out, token expired, or the token belongs to a different portal.
    Auth(String),
    /// The token file could not be read or written.
    Store(String),
}

impl std::fmt::Display for PortalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PortalError::Http(e) => write!(f, "portal unreachable: {e}"),
            PortalError::Contract(e) => write!(f, "portal protocol: {e}"),
            PortalError::Auth(e) => write!(f, "portal sign-in: {e}"),
            PortalError::Store(e) => write!(f, "portal token store: {e}"),
        }
    }
}

/// Resolve the portal base URL: `$GB_PORTAL` beats the config key, matching
/// how every other frontend knob layers (an explicit environment override
/// always wins). `None` means the portal is not configured at all — which is
/// a normal state, not an error.
pub fn portal_url(config_value: Option<&str>) -> Option<String> {
    let raw = std::env::var("GB_PORTAL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| config_value.map(str::to_string))?;
    let trimmed = raw.trim().trim_end_matches('/').to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_base_url_loses_its_trailing_slash() {
        // Joined with "/api/..." later; a kept slash would make "//api".
        assert_eq!(
            portal_url(Some("https://example.test/")).as_deref(),
            Some("https://example.test")
        );
    }

    #[test]
    fn no_configuration_is_none_not_an_error() {
        // GB_PORTAL may leak in from the session; only assert when it is absent.
        if std::env::var("GB_PORTAL").is_err() {
            assert_eq!(portal_url(None), None);
        }
    }
}
