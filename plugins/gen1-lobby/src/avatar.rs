//! The streaming layer: which of the people in the room become game objects.
//!
//! # Four numbers, and only two of them are hardware
//!
//! | Number | Value | Fixed by |
//! |---|---|---|
//! | connected to one room | no cartridge limit | our relay and our bookkeeping |
//! | held as game objects | 15 (16 slots, slot 0 is the player) | `wSpriteStateData` |
//! | drawn at once | 10 characters including you | 40 OAM entries / 4 per character |
//! | on one horizontal band | 5 characters including you | the DMG's 10-objects-per-scanline rule |
//!
//! The cartridge does not recycle its object slots — a Gen 1 map's object list
//! is fixed at load — so the *streaming* is the one mechanism the crowd needs
//! that the engine does not already provide. It is this file, and it is short.
//!
//! # Why nine and not fifteen
//!
//! `PrepareOAMData` walks all sixteen slots and writes four OAM entries each
//! with **no bounds check**, and `wShadowOAMEnd` *is* `wTileMap` — so the
//! eleventh drawn character writes its OAM entries straight into the on-screen
//! tile buffer. Capping remote avatars at nine makes that impossible by
//! construction rather than merely unlikely: a crowd gathering around you can
//! never trash the screen. Slots 10-15 stay free for anything else the room
//! wants.

use crate::gen1::*;
use terminalgb_plugin_api::Machine;

/// The most remote avatars that may be game objects at once. Nine plus the
/// player is ten, which is exactly the OAM budget.
pub const MAX_AVATARS: usize = 9;

/// How many pixels the engine shifts every sprite it maintains, per frame of
/// the player's walk, in the direction opposite the player's motion
/// (`AdvancePlayerSprite`'s `.scrollBackgroundAndSprites`: the step vector is
/// ±1 and it is `sla`'d once). Eight frames at two pixels is one 16-pixel walk
/// tile, which is what makes [`slot_bytes`] and the engine agree at every step
/// boundary — see `the_engines_own_shift_lands_where_we_would_have_recomputed`.
pub const ENGINE_SHIFT_PER_FRAME: i32 = 2;
/// Frames in one walk step, i.e. `wWalkCounter`'s reload value.
pub const WALK_FRAMES: i32 = 8;

/// **Is the engine moving every sprite this frame?**
///
/// `wYCoord`/`wXCoord` do not move until a step *finishes*, so recomputing an
/// avatar's screen pixels part-way through one writes back the position it had
/// before the step began — undoing the shift the engine just applied, and
/// pinning the avatar to the screen so that it appears to travel with the
/// player and then snap back. That is the whole of the "moving also moves the
/// other player, then they auto-correct" report.
///
/// So while this is true the avatars are the engine's, and we do not touch
/// them. It costs at most one 133 ms step of staleness in a crowd that is
/// already sampled at 8 Hz.
pub fn engine_owns_sprite_positions(m: &mut Machine<'_>) -> bool {
    m.peek(W_WALK_COUNTER).unwrap_or(0) != 0
}

/// Tell the engine how many sprite slots this map has, and give each of them a
/// text ID that is real.
///
/// `.spriteShiftLoop` is gated on `wNumSprites` alone and both Cable Club maps
/// declare **one** object, so an avatar in slot 2 or beyond was never shifted
/// at all. Raising the count also puts our avatars inside
/// `IsSpriteInFrontOfPlayer` — which is where their solidity comes from, and
/// also where an A-press turns a slot number into a text ID — so the text IDs
/// have to be filled in at the same time or talking to the ninth person in the
/// room walks off the end of the map's text-pointer table.
pub fn hand_slots_to_the_engine(m: &mut Machine<'_>, avatars: usize) {
    // Never below one: the cartridge's own opponent object is slot 1 and the
    // engine was already maintaining it.
    let n = avatars.clamp(1, MAX_AVATARS).min(MAX_OBJECT_EVENTS);
    m.poke(W_NUM_SPRITES, n as u8);
    for slot in 1..=n {
        // (movement byte 2, text ID); `DisplayTextID` reads the second.
        let e = W_MAP_SPRITE_DATA + (slot as u16 - 1) * 2;
        m.poke(e + 1, OPPONENT_TEXT_ID);
    }
}

/// Someone else in the room, as the relay describes them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub id: u32,
    pub name: String,
    pub x: u8,
    pub y: u8,
    pub facing: Facing,
    /// Walk-animation frame 0-3. Standing is 0; a mover cycles.
    pub step: u8,
    /// What they are saying, if anything.
    pub says: Option<String>,
}

impl Peer {
    pub fn new(id: u32, name: &str, x: u8, y: u8) -> Peer {
        Peer {
            id,
            name: name.to_string(),
            x,
            y,
            facing: Facing::Down,
            step: 0,
            says: None,
        }
    }
}

/// Where the local player is, as the game has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Presence {
    pub x: u8,
    pub y: u8,
    pub facing: Facing,
}

impl Presence {
    pub fn read(m: &mut Machine<'_>) -> Presence {
        Presence {
            x: m.peek(W_X_COORD).unwrap_or(0),
            y: m.peek(W_Y_COORD).unwrap_or(0),
            facing: Facing::from_byte(
                m.peek(W_SPRITE_STATE_DATA1 + S1_FACING).unwrap_or(0),
            ),
        }
    }
}

/// **The engine's own visibility window**, transcribed from
/// `CheckSpriteAvailability` (`engine/overworld/movement.asm`): a sprite is
/// culled unless its `MAPY` is in `[wYCoord, wYCoord + SCREEN_HEIGHT/2 - 1]`
/// and its `MAPX` in `[wXCoord, wXCoord + SCREEN_WIDTH/2 - 1]`. Sprite map
/// coordinates carry the `+4` offset, so in walk tiles that is four up, four
/// down, four left and five right of the player.
///
/// We reimplement it rather than inherit it because leaving
/// `IMAGEBASEOFFSET` at zero — the thing that stops the movement AI walking our
/// avatars off on their own — also stops `_UpdateSprites` running the cull. The
/// trade is deliberate: we get exact control and we owe the caller this
/// function.
pub fn visible(me: &Presence, p: &Peer) -> bool {
    let (my, mx) = (me.y as i32, me.x as i32);
    let (py, px) = (p.y as i32, p.x as i32);
    let (sy, sx) = (py + SPRITE_MAP_OFFSET as i32, px + SPRITE_MAP_OFFSET as i32);
    (my..=my + (SCREEN_H as i32 / 2 - 1)).contains(&sy)
        && (mx..=mx + (SCREEN_W as i32 / 2 - 1)).contains(&sx)
}

/// Squared distance from the player, in walk tiles. Used only for ranking, so
/// the square root is wasted work.
fn dist2(me: &Presence, p: &Peer) -> i32 {
    let dx = p.x as i32 - me.x as i32;
    let dy = p.y as i32 - me.y as i32;
    dx * dx + dy * dy
}

/// Choose which peers become objects this tick: the visible ones, nearest
/// first, capped at [`MAX_AVATARS`]. Ties break on `id` so the choice is
/// stable frame to frame and an avatar does not flicker between two slots.
pub fn choose<'a>(me: &Presence, peers: &'a [Peer]) -> Vec<&'a Peer> {
    let mut near: Vec<&Peer> = peers.iter().filter(|p| visible(me, p)).collect();
    near.sort_by_key(|p| (dist2(me, p), p.id));
    near.truncate(MAX_AVATARS);
    near
}

/// The one avatar appearance every remote player wears.
///
/// `LoadMapSpriteTilePatterns` de-duplicates by picture id, so a hundred
/// players all wearing the Trade Center opponent's sprite cost **one** of the
/// ten VRAM pattern slots — and that slot is already resident, because the
/// cartridge's own `TradeCenter_Script` put a `SPRITE_RED` in object 1. So a
/// remote avatar costs no VRAM work at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub picture_id: u8,
    /// The high nibble of `IMAGEINDEX`: which VRAM pattern slot to draw from.
    pub vram_slot: u8,
}

impl Default for Appearance {
    fn default() -> Appearance {
        Appearance {
            picture_id: 1,
            vram_slot: 1,
        }
    }
}

impl Appearance {
    /// Read the cartridge's own opponent object, so we wear whatever it wears
    /// rather than guessing an id. Falls back to the default if the room has
    /// not placed it yet.
    pub fn from_slot1(m: &mut Machine<'_>) -> Appearance {
        let base = W_SPRITE_STATE_DATA1 + SPRITE_STRUCT_LEN;
        let picture_id = m.peek(base + S1_PICTURE_ID).unwrap_or(0);
        let image_index = m.peek(base + S1_IMAGE_INDEX).unwrap_or(0);
        if picture_id == 0 || image_index == 0xFF {
            return Appearance::default();
        }
        Appearance {
            picture_id,
            vram_slot: image_index >> 4,
        }
    }

    /// `IMAGEINDEX` for a facing and a walk frame. The low nibble indexes
    /// `SpriteFacingAndAnimationTable` in groups of four
    /// (`data/sprites/facings.asm`): down 0-3, up 4-7, left 8-11, right 12-15,
    /// where "right" is the left art drawn through `.FlippedOAM`.
    pub fn image_index(&self, facing: Facing, step: u8) -> u8 {
        (self.vram_slot << 4) | (facing.as_byte() + (step & 3))
    }
}

/// The exact bytes one avatar costs. Pulled out of [`write_slot`] so a test can
/// assert the arithmetic without a machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotWrite {
    pub picture_id: u8,
    pub image_index: u8,
    pub y_pixels: u8,
    pub x_pixels: u8,
    pub facing: u8,
    pub map_y: u8,
    pub map_x: u8,
}

/// `InitializeSpriteScreenPosition` (`engine/overworld/movement.asm`) is the
/// authority for the two pixel fields, and it is worth reading as arithmetic
/// rather than trusting:
///
/// ```text
/// YPIXELS = (MAPY - wYCoord) * 16 - 4
/// XPIXELS = (MAPX - wXCoord) * 16
/// ```
///
/// with `MAPY`/`MAPX` carrying the `+4` offset. Put a peer on the player's own
/// square and it reduces to `(4*16 - 4, 4*16)` = `($3C, $40)` — which is
/// exactly the fixed screen position `ResetPlayerSpriteData` gives the player.
/// That coincidence is the check that the formula is the right one.
pub fn slot_bytes(look: &Appearance, me: &Presence, p: &Peer) -> SlotWrite {
    let map_y = p.y.wrapping_add(SPRITE_MAP_OFFSET);
    let map_x = p.x.wrapping_add(SPRITE_MAP_OFFSET);
    SlotWrite {
        picture_id: look.picture_id,
        image_index: look.image_index(p.facing, p.step),
        y_pixels: map_y.wrapping_sub(me.y).wrapping_mul(16).wrapping_sub(4),
        x_pixels: map_x.wrapping_sub(me.x).wrapping_mul(16),
        facing: p.facing.as_byte(),
        map_y,
        map_x,
    }
}

fn write_slot(m: &mut Machine<'_>, slot: u8, w: &SlotWrite) {
    let b1 = W_SPRITE_STATE_DATA1 + SPRITE_STRUCT_LEN * slot as u16;
    let b2 = W_SPRITE_STATE_DATA2 + SPRITE_STRUCT_LEN * slot as u16;
    m.poke(b1 + S1_PICTURE_ID, w.picture_id);
    m.poke(b1 + S1_IMAGE_INDEX, w.image_index);
    m.poke(b1 + S1_Y_PIXELS, w.y_pixels);
    m.poke(b1 + S1_X_PIXELS, w.x_pixels);
    m.poke(b1 + S1_FACING, w.facing);
    m.poke(b2 + S2_MAP_Y, w.map_y);
    m.poke(b2 + S2_MAP_X, w.map_x);
    // Zero, and it must stay zero: `_UpdateSprites` skips a slot whose
    // `IMAGEBASEOFFSET` is zero, so no random-walk AI ever moves our avatar and
    // no cull ever overwrites the `IMAGEINDEX` we just chose.
    m.poke(b2 + S2_IMAGE_BASE_OFFSET, 0);
}

fn clear_slot(m: &mut Machine<'_>, slot: u8) {
    let b1 = W_SPRITE_STATE_DATA1 + SPRITE_STRUCT_LEN * slot as u16;
    let b2 = W_SPRITE_STATE_DATA2 + SPRITE_STRUCT_LEN * slot as u16;
    // `PrepareOAMData` gates on `PICTUREID` alone, so this one byte is the
    // whole of "not here".
    m.poke(b1 + S1_PICTURE_ID, 0);
    m.poke(b2 + S2_IMAGE_BASE_OFFSET, 0);
}

/// Write this tick's crowd into sprite slots 1..=9 and clear 10..15.
///
/// Returns the ids that became objects, nearest first — which is what a status
/// line should show, because "who can I see?" is the question a player in a
/// crowd actually has.
pub fn stream(
    m: &mut Machine<'_>,
    look: &Appearance,
    me: &Presence,
    peers: &[Peer],
) -> Vec<u32> {
    let chosen = choose(me, peers);
    let ids: Vec<u32> = chosen.iter().map(|p| p.id).collect();
    let writes: Vec<SlotWrite> = chosen.iter().map(|p| slot_bytes(look, me, p)).collect();
    for (i, w) in writes.iter().enumerate() {
        write_slot(m, 1 + i as u8, w);
    }
    for slot in (1 + writes.len() as u8)..SPRITE_SLOTS {
        clear_slot(m, slot);
    }
    hand_slots_to_the_engine(m, writes.len());
    ids
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me() -> Presence {
        Presence {
            x: 10,
            y: 10,
            facing: Facing::Down,
        }
    }

    #[test]
    fn a_peer_on_my_own_square_lands_on_the_players_own_fixed_pixels() {
        // $3C / $40 are what `ResetPlayerSpriteData` writes for the player.
        let w = slot_bytes(&Appearance::default(), &me(), &Peer::new(1, "A", 10, 10));
        assert_eq!((w.y_pixels, w.x_pixels), (0x3C, 0x40));
    }

    #[test]
    fn a_peer_one_tile_right_is_sixteen_pixels_right() {
        let w = slot_bytes(&Appearance::default(), &me(), &Peer::new(1, "A", 11, 10));
        assert_eq!(w.x_pixels, 0x40 + 16);
        assert_eq!(w.y_pixels, 0x3C);
        assert_eq!((w.map_x, w.map_y), (15, 14));
    }

    /// **The invariant the whole "leave it to the engine" fix rests on.**
    ///
    /// A standing avatar's screen pixels are the engine's between step
    /// boundaries: it subtracts two per frame for the eight frames of a walk
    /// (`AdvancePlayerSprite`'s `.spriteShiftLoop`). At the end of the step
    /// `wYCoord`/`wXCoord` finally move, and what [`slot_bytes`] then
    /// recomputes must be **byte for byte** what the engine arrived at —
    /// otherwise handing it over would trade a drift for a jump.
    ///
    /// Measured on the cartridge as well as asserted here: slot 1's `YPIXELS`
    /// walked `$10 $0e $0c … $fc` across one southward step while `wYCoord`
    /// stayed put, then `wYCoord` incremented on the frame the counter read 0.
    #[test]
    fn the_engines_own_shift_lands_where_we_would_have_recomputed() {
        let look = Appearance::default();
        let peer = Peer::new(1, "B", 12, 8);
        for (dx, dy) in [(0i32, 1i32), (0, -1), (1, 0), (-1, 0)] {
            let before = Presence {
                x: 10,
                y: 10,
                facing: Facing::Down,
            };
            let after = Presence {
                x: (before.x as i32 + dx) as u8,
                y: (before.y as i32 + dy) as u8,
                facing: Facing::Down,
            };
            let start = slot_bytes(&look, &before, &peer);
            let end = slot_bytes(&look, &after, &peer);
            // What the engine does over the step, one frame at a time.
            let shift = ENGINE_SHIFT_PER_FRAME * WALK_FRAMES;
            let shifted_y = (start.y_pixels as i32 - dy * shift) as u8;
            let shifted_x = (start.x_pixels as i32 - dx * shift) as u8;
            assert_eq!((shifted_y, shifted_x), (end.y_pixels, end.x_pixels));
        }
    }

    /// A room with nobody else in it still leaves the engine one slot to
    /// maintain — the cartridge's own opponent object — and a full room never
    /// claims more slots than there are.
    #[test]
    fn the_slot_count_handed_over_is_never_zero_and_never_past_the_end() {
        for avatars in 0..=20usize {
            let n = avatars.clamp(1, MAX_AVATARS).min(MAX_OBJECT_EVENTS);
            assert!((1..=MAX_AVATARS).contains(&n), "{avatars} -> {n}");
        }
    }

    #[test]
    fn the_crowd_can_never_exceed_the_oam_budget() {
        // Twenty people standing on top of each other is the worst case the
        // §1.2 measurement says corrupts the screen at eleven characters.
        let peers: Vec<Peer> = (0..20).map(|i| Peer::new(i, "X", 10, 10)).collect();
        assert_eq!(choose(&me(), &peers).len(), MAX_AVATARS);
    }

    #[test]
    fn the_nearest_people_are_the_ones_you_see() {
        let mut peers: Vec<Peer> = Vec::new();
        // Twelve at increasing distance, all inside the window.
        for i in 0..12u32 {
            peers.push(Peer::new(i, "X", 10, 10u8.saturating_sub(i as u8 / 4)));
        }
        peers.push(Peer::new(99, "FAR", 13, 13));
        let chosen = choose(&me(), &peers);
        assert_eq!(chosen.len(), MAX_AVATARS);
        assert!(!chosen.iter().any(|p| p.id == 99));
    }

    #[test]
    fn the_visibility_window_is_the_engines_own() {
        let m = me();
        // Four up, four down, four left, five right — measured off
        // CheckSpriteAvailability's SCREEN_HEIGHT/2 - 1 and SCREEN_WIDTH/2 - 1.
        assert!(visible(&m, &Peer::new(0, "", 10, 6)));
        assert!(!visible(&m, &Peer::new(0, "", 10, 5)));
        assert!(visible(&m, &Peer::new(0, "", 10, 14)));
        assert!(!visible(&m, &Peer::new(0, "", 10, 15)));
        assert!(visible(&m, &Peer::new(0, "", 6, 10)));
        assert!(!visible(&m, &Peer::new(0, "", 5, 10)));
        assert!(visible(&m, &Peer::new(0, "", 15, 10)));
        assert!(!visible(&m, &Peer::new(0, "", 16, 10)));
    }

    #[test]
    fn the_image_index_walks_the_facing_table_in_fours() {
        let a = Appearance {
            picture_id: 1,
            vram_slot: 1,
        };
        assert_eq!(a.image_index(Facing::Down, 0), 0x10);
        assert_eq!(a.image_index(Facing::Up, 1), 0x15);
        assert_eq!(a.image_index(Facing::Left, 0), 0x18);
        assert_eq!(a.image_index(Facing::Right, 3), 0x1F);
        // The frame must never carry into the VRAM-slot nibble.
        assert_eq!(a.image_index(Facing::Right, 200) >> 4, 1);
    }

    #[test]
    fn a_peer_is_never_confused_with_the_off_screen_sentinel() {
        // $FF means "do not draw"; no facing/frame combination may produce it
        // for any of the ten VRAM slots.
        for slot in 0..10u8 {
            let a = Appearance {
                picture_id: 1,
                vram_slot: slot,
            };
            for f in [Facing::Down, Facing::Up, Facing::Left, Facing::Right] {
                for step in 0..4 {
                    assert_ne!(a.image_index(f, step), 0xFF);
                }
            }
        }
    }

    #[test]
    fn the_choice_is_stable_so_an_avatar_does_not_flicker_between_slots() {
        let peers: Vec<Peer> = (0..12u32).map(|i| Peer::new(i, "X", 10, 10)).collect();
        let a: Vec<u32> = choose(&me(), &peers).iter().map(|p| p.id).collect();
        let b: Vec<u32> = choose(&me(), &peers).iter().map(|p| p.id).collect();
        assert_eq!(a, b);
    }
}
