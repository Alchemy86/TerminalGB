//! The offer **card** — everything the portal needs to draw a real card for
//! an offered Pokémon, read or derived from the owner's own cartridge at the
//! moment of the offer.
//!
//! The captain's complaint was a one-line text pill ("SPARK lv12 (Pikachu)")
//! where a card belongs: sprite, types, moves, the DV detail the trainer
//! panel computes. The trade spec ([`MonSpec`]) already carries the moves and
//! DVs; what it cannot carry is anything that is not *trade* data — so those
//! display extras travel beside the mon, in a `card` object built here.
//!
//! **The sprite rule, unchanged and non-negotiable:** game artwork is never
//! shipped, vendored or served from this repository. The sprite on the wire
//! is decompressed from the *user's own cartridge* at runtime by
//! [`terminalgb_plugin_trainer::sprite`] (proven byte-identical to the
//! `pret/pokered` disassembly's 2bpp output), with the page behind the figure
//! made transparent by the same edge flood fill the trainer panel uses. A
//! species whose picture cannot be decoded sends **no** sprite — an honest
//! absence, never a placeholder.
//!
//! [`MonSpec`]: terminalgb_plugin_gen1_link::spec::MonSpec

use terminalgb_plugin_trainer::collection::Offerable;
use terminalgb_plugin_trainer::rom::RomTables;
use terminalgb_plugin_trainer::{derived, sprite};

/// The front sprite as a small PNG: greyscale DMG shades, transparent
/// background, typically ~1 KB before base64. Small on purpose — it is a
/// Gen 1 sprite, not a photograph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardSprite {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Display-only extras for one offered Pokémon. Everything here is either
/// read from the cartridge's own tables (sprite, types) or derived from the
/// spec's DVs by the trainer panel's one implementation (DV total, Hidden
/// Power) — nothing is typed and nothing is an input the portal should trust
/// over the `dvs` beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferCard {
    pub sprite: Option<CardSprite>,
    /// Type names from the cartridge's own `TypeNames` table; one entry when
    /// both slots agree (most species), two otherwise.
    pub types: Vec<String>,
    /// Sum of all five DVs (the derived HP DV included), 0..=75 — the number
    /// the picker and the trainer panel already show.
    pub dv_total: u8,
    /// What Hidden Power would be **if traded to Gen 2** — the move does not
    /// exist in Gen 1, same wording rule as the shiny flag.
    pub hidden_power_type: String,
    pub hidden_power_power: u8,
}

/// Which save, on which game, this offer belongs to — so a player connecting
/// with Blue and then Gold sees two collections, not one muddled list. The
/// natural key is the game title plus the save's player identity (name +
/// 16-bit trainer id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfferGame {
    /// e.g. "Pokémon Blue" — [`terminalgb_plugin_trainer::Game::label`].
    pub title: String,
    pub player: String,
    pub player_id: u16,
}

/// Everything an offer POST carries beside the `mon` spec. Each part is
/// optional on the wire so an older client (or a source with no cartridge in
/// hand) still posts a valid offer.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OfferExtras {
    pub card: Option<OfferCard>,
    pub game: Option<OfferGame>,
    /// The per-save identity heuristic — see [`super::offer::identity_for`].
    pub identity: Option<String>,
}

/// The DMG greys the emulator's default palette uses (lightest first, index
/// 0..=3 as the 2bpp decoder emits them). A neutral choice for a card drawn
/// on an unknown page background; the transparency is what matters.
const SHADES: [[u8; 3]; 4] = [
    [0xff, 0xff, 0xff],
    [0xc0, 0xc0, 0xc0],
    [0x60, 0x60, 0x60],
    [0x00, 0x00, 0x00],
];

/// Build the card for one owned Pokémon from the cartridge's own tables.
///
/// The sprite path is the trainer panel's: decompress the front picture from
/// the ROM, mask the page out with the edge flood fill (a colour key would
/// punch holes through eyes and bellies — the panel's hard-won rule), and
/// encode the RGBA as a PNG whose transparent pixels keep their opaque RGB
/// (a consumer that drops alpha degrades to an opaque box, not garbage).
pub fn card_for(e: &Offerable, tables: &RomTables) -> OfferCard {
    let sprite = tables.front_sprite(e.species).map(|(px, w, h)| {
        let bg = sprite::background_mask(&px, w, h);
        let mut rgba = Vec::with_capacity(px.len() * 4);
        for (i, &p) in px.iter().enumerate() {
            let [r, g, b] = SHADES[(p & 3) as usize];
            let a = if bg.get(i).copied().unwrap_or(false) {
                0
            } else {
                255
            };
            rgba.extend_from_slice(&[r, g, b, a]);
        }
        let mut png = Vec::new();
        crate::termimage::png_into(&rgba, w as u32, h as u32, &mut png);
        CardSprite {
            png,
            width: w as u32,
            height: h as u32,
        }
    });
    let types = match tables.base_types(e.species) {
        Some((t1, t2)) => {
            let a = tables.type_name(t1);
            let b = if t2 == t1 {
                String::new()
            } else {
                tables.type_name(t2)
            };
            [a, b].into_iter().filter(|t| !t.is_empty()).collect()
        }
        None => Vec::new(),
    };
    let hp = derived::hidden_power(e.dvs);
    OfferCard {
        sprite,
        types,
        dv_total: e.dv_total(),
        hidden_power_type: hp.type_name.to_string(),
        hidden_power_power: hp.power,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_trainer::collection::Location;
    use terminalgb_plugin_trainer::gen1::Dvs;

    fn entry() -> Offerable {
        Offerable {
            location: Location::Party(0),
            species: 0x99,
            species_name: "ABRA".into(),
            dex: 63,
            nickname: "GLIMMER".into(),
            level: 9,
            ot_name: "RED".into(),
            ot_id: 43981,
            moves: vec!["TELEPORT".into()],
            dvs: Dvs {
                attack: 14,
                defense: 10,
                speed: 10,
                special: 10,
            },
            shiny_if_traded: true,
            refusal: None,
        }
    }

    /// A cartridge the tables cannot read (an empty ROM) yields a card with
    /// no sprite and no types — honest absences, never a placeholder — while
    /// the DV-derived numbers still ride, because they come from the spec.
    #[test]
    fn an_unreadable_cartridge_sends_absences_not_placeholders() {
        let tables = RomTables::new(vec![0; 0x8000]);
        let card = card_for(&entry(), &tables);
        assert!(card.sprite.is_none());
        assert!(card.types.is_empty());
        // HP DV is the low bit of each of the four: all even here, so 0.
        assert_eq!(card.dv_total, 14 + 10 + 10 + 10);
        assert!(!card.hidden_power_type.is_empty());
        assert!((31..=70).contains(&card.hidden_power_power));
    }
}
