//! The HTTP contract, as the emulator speaks it.
//!
//! The route list is the agreement with the portal repo (its
//! `docs/api-contract.md`); the JSON bodies this module reads and writes are
//! documented field by field in `docs/portal.md` here. A Pokémon on the wire
//! is the JSON spelling of [`MonSpec`] — the same fields as a `.trade` file,
//! because the whole design is that the portal carries a *specification*, not
//! game bytes.
//!
//! Parsing is deliberately strict about what matters (a level of 0, a fifth
//! move, a DV of 16 are refused **by field name**) and liberal about what does
//! not (an id may be a string or a number). A refused Pokémon is refused here,
//! at fetch time, loudly — never discovered mid-trade.

use super::{auth, http, PortalError};
use terminalgb_plugin_agent::json::{self, Json, Writer};
use terminalgb_plugin_gen1_link::spec::MonSpec;

/// A signed-in connection to one portal.
pub struct Portal {
    base: String,
    token: auth::Token,
}

/// One trade the portal has matched and this player has not completed yet:
/// what to arm the local link partner with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingTrade {
    pub id: String,
    /// The friend on the other side, for display.
    pub friend: String,
    /// What this player agreed to give, as display text (e.g. "CHARIZARD
    /// lv36"). Confirmation wording only — the give side never travels from
    /// here, it was named when the offer was made.
    pub give: String,
    /// The partner's trainer name shown in-game during the trade.
    pub trainer: String,
    /// The Pokémon that will arrive.
    pub receive: MonSpec,
}

impl Portal {
    /// Open a connection: resolve the stored token against this portal URL.
    /// Signed out (or a token for a different portal) is an [`PortalError::Auth`]
    /// that says how to fix it.
    pub fn connect(base: &str) -> Result<Portal, PortalError> {
        match auth::load(base)? {
            auth::Stored::Token(token) => Ok(Portal {
                base: base.to_string(),
                token,
            }),
            auth::Stored::None => Err(PortalError::Auth(
                "not signed in — run `terminal login` (or press @ in the TUI)".into(),
            )),
            auth::Stored::Mismatch { stored_portal } => Err(PortalError::Auth(format!(
                "the stored sign-in belongs to {stored_portal}, not {base} — \
                 run `terminal login` again"
            ))),
        }
    }

    /// The signed-in account name, when the portal reported one at sign-in.
    pub fn user(&self) -> Option<&str> {
        self.token.user.as_deref()
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str) -> Result<http::Response, PortalError> {
        let resp = http::get(&format!("{}{path}", self.base), Some(&self.token.access))?;
        self.check(resp, path)
    }

    fn post(&self, path: &str, body: &str) -> Result<http::Response, PortalError> {
        let resp = http::post_json(
            &format!("{}{path}", self.base),
            Some(&self.token.access),
            body,
        )?;
        self.check(resp, path)
    }

    fn check(
        &self,
        resp: http::Response,
        path: &str,
    ) -> Result<http::Response, PortalError> {
        if resp.status == 401 || resp.status == 403 {
            return Err(PortalError::Auth(
                "the portal no longer accepts this sign-in — run `terminal login` again"
                    .into(),
            ));
        }
        if !resp.ok() {
            return Err(PortalError::Contract(format!(
                "{path} answered {}: {}",
                resp.status,
                resp.body.trim()
            )));
        }
        Ok(resp)
    }

    /// `GET /api/trades/mine` — every matched trade waiting on this player.
    pub fn trades_mine(&self) -> Result<Vec<PendingTrade>, PortalError> {
        let resp = self.get("/api/trades/mine")?;
        let j = json::parse(&resp.body).map_err(|e| {
            PortalError::Contract(format!("trades/mine is not JSON: {e}"))
        })?;
        let arr = j.get("trades").and_then(Json::as_array).ok_or_else(|| {
            PortalError::Contract("trades/mine carries no 'trades' array".into())
        })?;
        // The portal's trades/mine includes recently *completed* trades and
        // ones this side has already confirmed (`youDone`) — arming either
        // would replay a finished trade, so both are dropped here.
        arr.iter()
            .filter(|t| {
                let status = t.str_field("status").unwrap_or("accepted");
                let done = matches!(t.get("youDone"), Some(Json::Bool(true)));
                status == "accepted" && !done
            })
            .map(pending_from_json)
            .collect()
    }

    /// `POST /api/trades/<id>/done` — tell the portal this side's trade
    /// completed against the local partner.
    pub fn trade_done(&self, id: &str) -> Result<(), PortalError> {
        self.post(&format!("/api/trades/{id}/done"), "{}")?;
        Ok(())
    }

    /// `POST /api/offers` — put one Pokémon up for a named friend to see.
    /// Returns the offer id.
    pub fn offer(&self, mon: &MonSpec) -> Result<String, PortalError> {
        let mut w = Writer::new();
        w.begin_obj();
        w.obj_field("mon");
        mon_to_json(&mut w, mon);
        w.end_obj();
        w.end_obj();
        let resp = self.post("/api/offers", &w.finish())?;
        let j = json::parse(&resp.body).map_err(|e| {
            PortalError::Contract(format!("offers reply is not JSON: {e}"))
        })?;
        // The contract wraps the created offer (`{"offer":{"id":…}}`); a bare
        // top-level id is accepted too.
        j.get("offer")
            .and_then(|o| id_field(o, "id"))
            .or_else(|| id_field(&j, "id"))
            .ok_or_else(|| PortalError::Contract("offers reply carries no 'id'".into()))
    }

    /// `GET /api/friends/<user>/offers` — what a friend is offering, as
    /// display lines (the arriving Pokémon's full spec only travels once a
    /// trade is matched).
    pub fn friend_offers(
        &self,
        user: &str,
    ) -> Result<Vec<(String, String)>, PortalError> {
        let resp = self.get(&format!("/api/friends/{user}/offers"))?;
        let j = json::parse(&resp.body).map_err(|e| {
            PortalError::Contract(format!("friend offers are not JSON: {e}"))
        })?;
        let arr = j
            .get("offers")
            .and_then(Json::as_array)
            .ok_or_else(|| PortalError::Contract("no 'offers' array".into()))?;
        arr.iter()
            .map(|o| {
                let id = id_field(o, "id").ok_or_else(|| {
                    PortalError::Contract("an offer has no 'id'".into())
                })?;
                let summary = o
                    .str_field("summary")
                    .unwrap_or("(no description)")
                    .to_string();
                Ok((id, summary))
            })
            .collect()
    }

    /// `POST /api/offers/<id>/accept` — accept a friend's offer, naming the
    /// Pokémon given back.
    pub fn accept(&self, offer_id: &str, give: &MonSpec) -> Result<String, PortalError> {
        let mut w = Writer::new();
        w.begin_obj();
        // The contract names the give-back field "mon" (what you give back).
        w.obj_field("mon");
        mon_to_json(&mut w, give);
        w.end_obj();
        w.end_obj();
        let resp = self.post(&format!("/api/offers/{offer_id}/accept"), &w.finish())?;
        let j = json::parse(&resp.body).map_err(|e| {
            PortalError::Contract(format!("accept reply is not JSON: {e}"))
        })?;
        // Contract reply is `{"trade": Trade}`; older stubs said trade_id/id.
        j.get("trade")
            .and_then(|t| id_field(t, "id"))
            .or_else(|| id_field(&j, "trade_id"))
            .or_else(|| id_field(&j, "id"))
            .ok_or_else(|| {
                PortalError::Contract("accept reply carries no trade id".into())
            })
    }
}

/// An id may arrive as a JSON string or number; both read back as a string.
fn id_field(j: &Json, key: &str) -> Option<String> {
    match j.get(key)? {
        Json::Str(s) => Some(s.clone()),
        Json::Num(n) => Some(format!("{}", *n as u64)),
        _ => None,
    }
}

/// Encode one Pokémon spec into an open JSON object (caller owns the braces
/// via `obj_field`).
fn mon_to_json(w: &mut Writer, m: &MonSpec) {
    // The wire spellings are the portal contract's (`api-contract.md`):
    // camelCase OT fields and a dvs *object*. The snake_case twins are written
    // too, so an older stub or self-hosted portal keeps working — the portal
    // ignores fields it does not know.
    w.str_field("species", &m.species);
    w.num_field("level", m.level as f64);
    if !m.nickname.is_empty() {
        w.str_field("nickname", &m.nickname);
    }
    w.str_field("otName", &m.ot_name);
    w.num_field("otId", m.ot_id as f64);
    w.str_field("ot_name", &m.ot_name);
    w.num_field("ot_id", m.ot_id as f64);
    w.arr_field("moves");
    for mv in &m.moves {
        w.str_item(mv);
    }
    w.end_arr();
    w.obj_field("dvs");
    w.num_field("attack", m.dvs.0 as f64);
    w.num_field("defense", m.dvs.1 as f64);
    w.num_field("speed", m.dvs.2 as f64);
    w.num_field("special", m.dvs.3 as f64);
    w.end_obj();
    // Derived here from the DVs on every send — never accepted as an input —
    // so it can never disagree with the spec it rides on. The rule is the
    // trainer panel's one implementation (`derived::shiny_if_traded`,
    // pokecrystal `CheckShininess`), and the wording rule travels with it:
    // Gen 1 has no shiny Pokémon, this is "would be shiny IF TRADED to
    // Gen 2". The portal shows it in its lists; a portal wanting to verify it
    // recomputes the same derivation from `dvs`.
    w.bool_field(
        "shinyIfTradedToGen2",
        terminalgb_plugin_trainer::derived::shiny_if_traded(
            terminalgb_plugin_trainer::gen1::Dvs {
                attack: m.dvs.0,
                defense: m.dvs.1,
                speed: m.dvs.2,
                special: m.dvs.3,
            },
        ),
    );
}

/// Decode one Pokémon spec, refusing anything out of range **by field name**.
/// The ranges are the trade file parser's (`spec.rs`): the two paths must
/// accept the same Pokémon.
pub fn mon_from_json(j: &Json) -> Result<MonSpec, PortalError> {
    let bad = |m: String| PortalError::Contract(m);
    let species = j
        .str_field("species")
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("a Pokémon with no 'species'".into()))?
        .to_string();
    let level = j
        .u64_field("level")
        .ok_or_else(|| bad(format!("{species}: no 'level'")))?;
    if !(1..=100).contains(&level) {
        return Err(bad(format!("{species}: level {level} is outside 1..=100")));
    }
    let ot_id = j
        .u64_field("ot_id")
        .or_else(|| j.u64_field("otId"))
        .ok_or_else(|| bad(format!("{species}: no 'ot_id'")))?;
    if ot_id > u16::MAX as u64 {
        return Err(bad(format!(
            "{species}: ot_id {ot_id} does not fit 16 bits"
        )));
    }
    let moves: Vec<String> = match j.get("moves").and_then(Json::as_array) {
        Some(arr) => arr
            .iter()
            .map(|m| {
                m.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| bad(format!("{species}: a move that is not a string")))
            })
            .collect::<Result<_, _>>()?,
        None => Vec::new(),
    };
    if moves.len() > 4 {
        return Err(bad(format!(
            "{species}: {} moves — a Pokémon can know at most 4",
            moves.len()
        )));
    }
    // `dvs` arrives either as the trade-file array `[atk, def, spd, spc]` or
    // as the portal contract's object `{attack, defense, speed, special}`.
    let dvs_json = j
        .get("dvs")
        .ok_or_else(|| bad(format!("{species}: no 'dvs'")))?;
    let dv_values: Vec<Option<u64>> = match dvs_json {
        Json::Arr(arr) => arr.iter().map(Json::as_u64).collect(),
        Json::Obj(_) => ["attack", "defense", "speed", "special"]
            .iter()
            .map(|k| dvs_json.u64_field(k))
            .collect(),
        _ => return Err(bad(format!("{species}: 'dvs' is neither array nor object"))),
    };
    if dv_values.len() != 4 {
        return Err(bad(format!(
            "{species}: dvs needs four values (attack, defense, speed, special)"
        )));
    }
    let mut dvs = [0u8; 4];
    for (i, d) in dv_values.into_iter().enumerate() {
        let v = d
            .filter(|v| *v <= 15)
            .ok_or_else(|| bad(format!("{species}: DV #{} is outside 0..=15", i + 1)))?;
        dvs[i] = v as u8;
    }
    Ok(MonSpec {
        species,
        level: level as u8,
        nickname: j.str_field("nickname").unwrap_or("").to_string(),
        ot_name: j
            .str_field("ot_name")
            .or_else(|| j.str_field("otName"))
            .unwrap_or("PORTAL")
            .to_string(),
        ot_id: ot_id as u16,
        moves,
        dvs: (dvs[0], dvs[1], dvs[2], dvs[3]),
    })
}

fn pending_from_json(j: &Json) -> Result<PendingTrade, PortalError> {
    let id = id_field(j, "id")
        .ok_or_else(|| PortalError::Contract("a trade with no 'id'".into()))?;
    // Contract shape first (`youReceive`, `partner.displayName`, `youGive` as
    // a full spec); the flat `receive`/`friend`/`give` spelling of older
    // stubs is still read.
    let receive = j
        .get("youReceive")
        .or_else(|| j.get("receive"))
        .ok_or_else(|| {
            PortalError::Contract(format!("trade {id}: no 'youReceive' Pokémon"))
        })?;
    let partner_name = j
        .get("partner")
        .and_then(|p| p.str_field("displayName"))
        .or_else(|| j.str_field("friend"))
        .map(str::to_string);
    // The give side is confirmation text only. The contract sends the full
    // spec under `youGive`; summarise it the way the offer list does.
    let give = j
        .str_field("give")
        .map(str::to_string)
        .or_else(|| {
            let g = j.get("youGive")?;
            let name = g
                .str_field("nickname")
                .or_else(|| g.str_field("species"))?
                .to_string();
            let level = g.u64_field("level")?;
            Some(format!("{name} lv{level}"))
        })
        .unwrap_or_else(|| "(unknown)".to_string());
    Ok(PendingTrade {
        friend: partner_name
            .clone()
            .unwrap_or_else(|| "a friend".to_string()),
        give,
        trainer: j
            .str_field("trainer")
            .map(str::to_string)
            .or(partner_name)
            .unwrap_or_else(|| "PORTAL".to_string())
            .to_ascii_uppercase(),
        receive: mon_from_json(receive)
            .map_err(|e| PortalError::Contract(format!("trade {id}: {e}")))?,
        id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(s: &str) -> Json {
        json::parse(s).unwrap()
    }

    #[test]
    fn a_mon_round_trips_through_json() {
        let m = MonSpec {
            species: "Bulbasaur".into(),
            level: 14,
            nickname: "BULBA".into(),
            ot_name: "BEN".into(),
            ot_id: 12345,
            moves: vec!["Tackle".into(), "Growl".into()],
            dvs: (10, 8, 12, 15),
        };
        let mut w = Writer::new();
        w.begin_obj();
        mon_to_json(&mut w, &m);
        w.end_obj();
        let back = mon_from_json(&parse(&w.finish())).unwrap();
        assert_eq!(back, m);
    }

    /// The shiny prediction rides every outgoing mon, is derived from the DVs
    /// at send time (never accepted as an input, so it cannot disagree with
    /// them), and uses the one implementation the trainer panel uses.
    #[test]
    fn the_outgoing_mon_carries_the_shiny_if_traded_flag_derived_from_its_dvs() {
        let encode = |dvs: (u8, u8, u8, u8)| {
            let mut w = Writer::new();
            w.begin_obj();
            mon_to_json(
                &mut w,
                &MonSpec {
                    species: "Abra".into(),
                    dvs,
                    ..MonSpec::default()
                },
            );
            w.end_obj();
            w.finish()
        };
        // $EA $AA: attack 14 (bit 1 set), def/spd/spc all 10 — GLIMMER's
        // real spread, verified shiny in Gold (docs/link-cable.md §8).
        assert!(encode((14, 10, 10, 10)).contains("\"shinyIfTradedToGen2\":true"));
        // All-15s are NOT shiny — the classic wrong guess.
        assert!(encode((15, 15, 15, 15)).contains("\"shinyIfTradedToGen2\":false"));
    }

    #[test]
    fn out_of_range_fields_are_refused_by_name() {
        let e = mon_from_json(&parse(
            r#"{"species":"Mew","level":0,"ot_id":1,"dvs":[1,2,3,4]}"#,
        ))
        .unwrap_err();
        assert!(e.to_string().contains("level"), "{e}");
        let e = mon_from_json(&parse(
            r#"{"species":"Mew","level":5,"ot_id":1,"dvs":[16,2,3,4]}"#,
        ))
        .unwrap_err();
        assert!(e.to_string().contains("DV"), "{e}");
        let e = mon_from_json(&parse(
            r#"{"species":"Mew","level":5,"ot_id":1,"dvs":[1,2,3,4],
                "moves":["a","b","c","d","e"]}"#,
        ))
        .unwrap_err();
        assert!(e.to_string().contains("moves"), "{e}");
        let e = mon_from_json(&parse(
            r#"{"species":"Mew","level":5,"ot_id":99999,"dvs":[1,2,3,4]}"#,
        ))
        .unwrap_err();
        assert!(e.to_string().contains("ot_id"), "{e}");
    }

    #[test]
    fn a_trade_id_may_be_a_string_or_a_number() {
        let t = pending_from_json(&parse(
            r#"{"id":71,"friend":"ben","give":"CHARIZARD lv36",
                "receive":{"species":"Bulbasaur","level":14,"ot_id":1,"dvs":[1,2,3,4]}}"#,
        ))
        .unwrap();
        assert_eq!(t.id, "71");
        assert_eq!(t.trainer, "BEN"); // falls back to the friend, uppercased
        let t = pending_from_json(&parse(
            r#"{"id":"abc","trainer":"Ben",
                "receive":{"species":"Mew","level":5,"ot_id":1,"dvs":[1,2,3,4]}}"#,
        ))
        .unwrap();
        assert_eq!(t.id, "abc");
        assert_eq!(t.trainer, "BEN");
    }

    #[test]
    fn a_malformed_receive_names_the_trade_that_carried_it() {
        let e = pending_from_json(&parse(
            r#"{"id":9,"receive":{"species":"Mew","level":5,"ot_id":1,"dvs":[1,2,3]}}"#,
        ))
        .unwrap_err();
        assert!(e.to_string().contains("trade 9"), "{e}");
    }
}
