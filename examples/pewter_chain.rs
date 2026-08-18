//! **The Pewter chain** — twelve scenarios from a cold cartridge to Pewter
//! City, an expert solver that gets through them, and the corpus that comes out.
//!
//! Reader-facing page: [`docs/agent/pewter-chain.md`](../docs/agent/pewter-chain.md).
//!
//! ```text
//! cargo run --release --features scenario --example pewter_chain -- \
//!     chain --rom <blue.gb> --seeds 8
//! ```
//!
//! # What this is, and the one thing it is not
//!
//! [`ChainDriver`] is a **scripted solver**. It implements
//! [`terminalgb_plugin_agent::driver::Driver`] — the same seam a live model
//! plugs into, taking the same JSON snapshot and returning the same JSON plan
//! through the same parse-clamp-execute path — and it satisfies
//! `no_memory_writes` for the same reason a model would: it can only press
//! buttons. **It is not a model, and nothing it produces is evidence about
//! one.** It knows the route because a person wrote the route down.
//!
//! That is exactly the role the PokéAgent Challenge winner gave its scripted
//! policies: solve each subgoal, and distil the trajectories. Here the
//! trajectories are what is wanted; the distillation is somebody else's job.
//!
//! # Where the route came from
//!
//! Not from a walkthrough. Every waypoint is a turning point of a
//! breadth-first path over a walkability grid built from the cartridge's own
//! data — `maps/<Name>.blk` block ids, the tileset's blockset, and the
//! tileset's collision tile list — with the collision probe on the walk tile's
//! **bottom-left** 8x8 tile (block-definition indices 4, 6, 12, 14). Two
//! things fell out of that grid that a walkthrough would not have told you,
//! and both are load-bearing:
//!
//! * Route 1 northbound and southbound are **different paths**. Its ledges are
//!   one-way, so the climb is not the descent reversed.
//! * Route 2's southern half and its northern half are **not connected on
//!   foot** — separate components of the walkability graph. Viridian Forest is
//!   the only way between them, which is why the chain has to cross it.
//!
//! # Diversity is the product, so it is seeded and it is measured
//!
//! A corpus of one trajectory replayed teaches nothing. Every run takes a seed
//! and the seed moves: which of three preset names, which of three starters,
//! which Poké Ball on the table, which battle move, how many waypoints get
//! dropped so the executor has to path further on its own, and how far the
//! solver wanders before resuming. What that bought is measured in
//! `docs/agent/pewter-chain.md` rather than asserted here.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::exit;

use terminalgb::gameboy::Gameboy;
use terminalgb::plugin::Capability as Cap;
use terminalgb::scenario::record::Recording;
use terminalgb::scenario::run::ScenarioRun;
use terminalgb::scenario::Scenario;
use terminalgb_plugin_agent::driver::{Driver, Poll};
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::json;
use terminalgb_plugin_agent::{AgentPlugin, Config};

/// The rig's reusable half: the plan, the statistics and the report. Shared by
/// `#[path]` because examples are separate crates — the same arrangement
/// `examples/support/suite_baseline.rs` uses.
#[path = "support/abrig.rs"]
mod abrig;

// ---------------------------------------------------------------------------
// the route
// ---------------------------------------------------------------------------

/// What to do once the waypoints on a map have been walked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Finish {
    /// Arriving is the whole of it — a warp tile fires under your feet.
    Arrive,
    /// Hold a direction until the map changes. A map connection, and a Gen 1
    /// door: `CheckWarpsCollision` fires a door warp when you walk *into* the
    /// blocked tile, which is why the gates' row 0 is not walkable and is
    /// still the way out.
    Step(&'static str),
    /// Turn to face something and press A. Twice, because a press in a
    /// direction you are not facing only turns you.
    Talk(&'static str),
    /// Pace up and down a patch of tall grass until the goal fires.
    ///
    /// **This is what a remedial link looks like from the driver's side.** A
    /// scenario whose goal is a party read rather than a place has no
    /// destination to walk to; what it needs is to keep meeting wild Pokémon,
    /// and Gen 1 rolls an encounter per step taken on a grass tile.
    ///
    /// **A patrol between two anchors, not a direction.** Pacing by direction
    /// drifts: "walk up 3, walk down 3" is only symmetric while both are
    /// clear, and the first time one is walled the walk becomes one-way.
    /// Measured — 900 decisions of it left the player twenty-two tiles south
    /// of the grass it was supposed to be standing in.
    Grind((u8, u8), (u8, u8)),
}

/// One thing to do on one map: walk a chain of waypoints, then finish.
#[derive(Debug, Clone)]
struct Stage {
    via: Vec<(u8, u8)>,
    finish: Finish,
}

fn stage(via: &[(u8, u8)], finish: Finish) -> Stage {
    Stage {
        via: via.to_vec(),
        finish,
    }
}

/// **A map's stages are a program counter that survives leaving it.**
///
/// Viridian City is walked twice in one leg — once into the Pokémon Center and
/// once out of the top of the map — and "which of the two am I doing?" is not
/// answerable from the position, the party or the screen. So a completed stage
/// advances when the map is left, and coming back resumes at the next one. A
/// stage that was *not* completed (bumped into the wrong building, pushed out
/// by a script) does not advance, so an accident cannot skip a leg.
type MapPlan = Vec<Stage>;

const REDS_HOUSE_2F: u8 = 0x26;
const REDS_HOUSE_1F: u8 = 0x25;
const PALLET_TOWN: u8 = 0x00;
const OAKS_LAB: u8 = 0x28;
const ROUTE_1: u8 = 0x0C;
const VIRIDIAN_CITY: u8 = 0x01;
const VIRIDIAN_MART: u8 = 0x2A;
const ROUTE_2: u8 = 0x0D;
const FOREST_SOUTH_GATE: u8 = 0x32;
const VIRIDIAN_FOREST: u8 = 0x33;
const FOREST_NORTH_GATE: u8 = 0x2F;
const VIRIDIAN_POKECENTER: u8 = 0x29;
const PEWTER_CITY: u8 = 0x02;
const PEWTER_GYM: u8 = 0x36;
const PEWTER_POKECENTER: u8 = 0x3A;
/// Route 3, east of Pewter — where the chain goes next.
#[allow(dead_code)]
const ROUTE_3: u8 = 0x0E;

/// Route 1 climbing north. Turning points of the shortest walkable path; the
/// ledges make this different from the descent below.
const R1_NORTH: &[(u8, u8)] = &[
    (10, 28),
    (8, 28),
    (8, 24),
    (12, 24),
    (12, 20),
    (9, 20),
    (9, 14),
    (14, 14),
    (14, 2),
    (11, 2),
    (11, 0),
];
/// Route 1 going south. Not the reverse of the climb.
const R1_SOUTH: &[(u8, u8)] = &[
    (10, 4),
    (14, 4),
    (14, 18),
    (9, 18),
    (9, 22),
    (12, 22),
    (12, 26),
    (8, 26),
    (8, 31),
    (10, 31),
    (10, 35),
];
/// Pallet Town, wherever you start on it, up to the northern gap at x=10.
const PALLET_NORTH: &[(u8, u8)] = &[(9, 12), (9, 2), (10, 2), (10, 1)];
/// Pallet Town to the laboratory door at (12,11).
const PALLET_TO_LAB: &[(u8, u8)] = &[(10, 7), (9, 7), (9, 12), (12, 12), (12, 11)];
/// Route 2 southern half: down to the forest's south gate at (3,43).
const R2_TO_FOREST: &[(u8, u8)] = &[
    (8, 62),
    (7, 62),
    (7, 57),
    (5, 57),
    (5, 56),
    (4, 56),
    (4, 48),
    (8, 48),
    (8, 44),
    (3, 44),
    (3, 43),
];
/// Viridian Forest, south entrance to north exit. Sixteen turning points and
/// 131 steps: the longest single-map leg in the chain by a wide margin.
const FOREST: &[(u8, u8)] = &[
    (16, 44),
    (17, 44),
    (17, 43),
    (18, 43),
    (18, 41),
    (25, 41),
    (25, 12),
    (26, 12),
    (26, 9),
    (18, 9),
    (18, 16),
    (13, 16),
    (13, 3),
    (8, 3),
    (8, 22),
    (2, 22),
    (2, 19),
    (1, 19),
    (1, 1),
];

/// Pewter City, from where Route 2 lets you in, to the gym door at (16,17).
///
/// **Ten turning points for a walk of about thirty tiles**, and that is the
/// map rather than a bad path: column 18 is solid on rows 18, 19 and 20 and
/// row 21 is solid from x=0 to x=18, so the city's south-east quarter and its
/// south-west quarter meet only along row 13. Every route across Pewter goes
/// up and over.
const PEWTER_TO_GYM: &[(u8, u8)] = &[
    (16, 26),
    (16, 22),
    (19, 22),
    (19, 13),
    (11, 13),
    (11, 16),
    (10, 16),
    (10, 18),
    (16, 18),
    (16, 17),
];
/// The gym door back to the Pokémon Center door, which is **not** the reverse
/// of [`PEWTER_TO_GYM`] — it crosses row 13 the other way round.
const GYM_TO_POKECENTER: &[(u8, u8)] = &[
    (10, 18),
    (10, 13),
    (18, 13),
    (18, 17),
    (19, 17),
    (19, 26),
    (13, 26),
    (13, 25),
];
/// Out of the Pokémon Center and east off the map onto Route 3.
/// Out of the Pokémon Center and east off the map onto Route 3.
///
/// **Row 26 first, then straight up column 19.** The obvious diagonal is a
/// trap: column 18 is solid on rows 18, 19 and 20 and row 21 is solid from
/// x=0 to x=18, so the left half of rows 18-20 is a pocket with one way in and
/// out, and a `goto` heading north-east from the Pokémon Center walks into it.
/// Measured on the first 60-attempt run of this link: **23 of 23 failures were
/// stuck on row 20 between x=13 and x=17**, inside that pocket, with no battle
/// involved at all. The same shape as link 08's ledge — a waypoint whose
/// overshoot cannot be recovered from.
const PEWTER_TO_ROUTE_3: &[(u8, u8)] = &[(19, 26), (19, 18), (39, 18)];

/// Pewter's Pokémon Center door down to the southern edge, where the map
/// connection drops you onto the top of Route 2.
const PEWTER_TO_ROUTE_2: &[(u8, u8)] = &[(18, 31), (18, 35)];
/// The top of Route 2 into the grass block at x=0-7, y=2-7.
///
/// **Forty-eight tiles of grass a dozen steps from a Pokémon Center**, which
/// is what makes this the place to train for Brock rather than Route 1. Both
/// halves matter: Route 2's encounter table averages 29.4 experience against
/// Route 1's 25.0 (`Wild`, read from `WildDataPointers`), and — far more
/// importantly — the walk to a nurse and back is a dozen tiles here against
/// the length of Route 1 plus Viridian City. A remedial link is dominated by
/// its heal trips, not by its fights: link 11 aimed one level higher on
/// Route 1 and spent its whole 600,000-frame budget without arriving,
/// 30 attempts out of 30, ending in Viridian City every time.
const ROUTE_2_TO_GRASS: &[(u8, u8)] = &[(8, 3), (2, 3)];
/// Out of the grass and back north into Pewter.
const ROUTE_2_TO_PEWTER: &[(u8, u8)] = &[(4, 2), (8, 2), (8, 0)];

/// Up the gym to Brock, **round the junior trainer rather than through him**.
///
/// **The gym is two pinch points and everything follows from them.** Rows 3
/// and 9 are solid except x=4 and x=5, so the middle band (rows 4-8) is
/// entered and left through one two-tile gap at each end. The junior trainer
/// stands at (3,6) facing **right** — his object's direction byte is `$D3` —
/// so the straight climb up column 4 crosses his line at (4,6). Column 1 runs
/// from row 8 to row 4 and every tile of it is to his **left**, which a
/// rightward sight line cannot reach at any range.
///
/// **Measured both ways, 120 attempts each from the level-10 pool**: fighting
/// him reached the goal 39 times with **69 of 81 failures on his own tile**;
/// avoiding him reached it 44 times with every failure at Brock's. At that
/// level the gym's junior trainer killed more of the chain than Brock did.
const GYM_TO_BROCK: &[(u8, u8)] = &[(4, 8), (1, 8), (1, 4), (4, 4), (4, 2)];

/// The straight climb, which fights him. Kept because it is the measured
/// alternative rather than a hypothetical one, and swapping it in is one line.
/// His party is DIGLETT Lv11 and SANDSHREW Lv11 and he is worth 273
/// experience, so this is a real trade and not an obviously worse route — it
/// simply loses at the level the chain arrives with.
#[allow(dead_code)]
const GYM_TO_BROCK_FIGHTING_THE_TRAINER: &[(u8, u8)] = &[(4, 11), (4, 2)];

/// The Viridian Pokémon Center: heal, then leave.
///
/// The nurse stands at (3,1) behind a counter, and (3,2) is solid. Gen 1's
/// counter tiles are looked *through* — `DisplayTextID` reaches two tiles when
/// the tile in front is one of the tileset's counter tiles — so the place to
/// stand is (3,3) facing up, not next to her.
const POKECENTER: &[Stage2] = &[
    Stage2 {
        via: &[(3, 5), (3, 3)],
        finish: Finish::Talk("up"),
    },
    Stage2 {
        via: &[(3, 6), (3, 7)],
        finish: Finish::Step("down"),
    },
];

/// Just the way out of a Pokémon Center, for a link that starts inside one.
///
/// **Both heal links finish inside the building**, because that is the
/// strongest claim the cartridge supports: "the party is whole *and* you are
/// standing in the Pokémon Center" forces anyone who is hurt through the
/// nurse, and it cannot be satisfied by walking past the door. See
/// `docs/agent/brock.md` for what it still cannot prove.
const LEAVE_POKECENTER: &[Stage2] = &[Stage2 {
    via: &[(3, 6), (3, 7)],
    finish: Finish::Step("down"),
}];

/// A `const`-friendly [`Stage`]: the same thing with a borrowed waypoint list,
/// because a `Vec` cannot be built in a `const`.
struct Stage2 {
    via: &'static [(u8, u8)],
    finish: Finish,
}

impl Stage2 {
    fn to_vec(list: &[Stage2]) -> Vec<Stage> {
        list.iter()
            .map(|s| Stage {
                via: s.via.to_vec(),
                finish: s.finish,
            })
            .collect()
    }
}
/// The route for one scenario: what to do on each map it can be standing on.
fn route_for(scenario: &str, seed: u64) -> HashMap<u8, MapPlan> {
    let mut r: HashMap<u8, MapPlan> = HashMap::new();
    // Which Poké Ball to take. The three starters are the same three objects
    // on the table, so this is a one-tile difference that changes the whole
    // rest of the playthrough.
    let ball_x = 6 + (seed % 3) as u8;
    match scenario {
        "01-new-game" => {}
        "02-leave-the-house" => {
            r.insert(
                REDS_HOUSE_2F,
                vec![stage(&[(4, 6), (4, 1), (7, 1)], Finish::Arrive)],
            );
            r.insert(
                REDS_HOUSE_1F,
                vec![stage(&[(7, 7), (3, 7)], Finish::Step("down"))],
            );
        }
        "03-oak-takes-you-to-the-lab" => {
            r.insert(
                PALLET_TOWN,
                vec![stage(
                    &[(5, 6), (8, 6), (8, 2), (10, 2), (10, 1)],
                    Finish::Arrive,
                )],
            );
            r.insert(OAKS_LAB, vec![stage(&[], Finish::Arrive)]);
        }
        // **(4,4) is behind the rival, and reaching for it walks into a
        // cartridge soft-lock.** Oak's escort ends at (5,3); the rival stands
        // at (4,3), so the first press of a `goto (4,4)` only turns the player
        // and `GoTo` commits to a downward detour that overshoots row 4 into
        // row 6 — where `OaksLabPlayerDontGoAwayScript` fires. Standing in rows
        // 5-6 of the laboratory is what triggers Pokémon Blue's own
        // `PrepareOAMData` overflow (see `docs/gen1/sharp-edges.md`), and the
        // game then locks up for good. Measured: 53 of 600 attempts, every one
        // of them ending frozen at (5,5).
        //
        // (5,4) is the turning point of the real walkable path — one step down,
        // then straight along row 4 — so nothing detours and nothing goes south.
        "04-take-a-starter" => {
            r.insert(
                OAKS_LAB,
                vec![stage(&[(5, 4), (ball_x, 4)], Finish::Talk("up"))],
            );
        }
        // **The rival challenges you at `wYCoord == 6` and nowhere else.**
        // `OaksLabRivalChallengesPlayerScript` opens `ld a,[wYCoord] / cp 6 /
        // ret nz`, so standing by the table after taking a starter waits for
        // ever — measured, 250 decisions of nothing at (7,5). And **row 6 of
        // the laboratory is two tiles wide**: the block map makes `####..####`
        // of it, so only x=4 and x=5 exist and aiming at x=6 is a wall.
        // Diversity has to come from the approach, not from the tile.
        //
        // **And the approach may not be on row 4, because row 4 is where the
        // rival parks.** `OaksLabChoseStarterScript` walks him to the ball he
        // takes and leaves him standing under it — measured on the certified
        // link-04 exits, at (7,4), (8,4) and (6,4) for a player who took
        // Charmander, Squirtle and Bulbasaur — and a sprite is solid. The old
        // list held (8,4), so every attempt whose player took the Squirtle ball
        // *and* drew that approach aimed at the tile the rival was standing on:
        // seed ≡ 1 (mod 12), which is exactly the 50 of 600 that timed out,
        // deterministically, walking round and round a tile it could never
        // reach.
        //
        // Row 5 has no sprite on it at all, and every one of its ten tiles is
        // reachable in a single `goto` from all three starter positions
        // (checked, x = 0..9 from each). So the approaches live there.
        "05-the-rival-battle" => {
            let x = 4 + (seed % 2) as u8;
            let approach = [(2u8, 5u8), (8, 5), (4, 5), (6, 5)][(seed % 4) as usize];
            r.insert(OAKS_LAB, vec![stage(&[approach, (x, 6)], Finish::Arrive)]);
        }
        "06-cross-route-1" => {
            r.insert(
                OAKS_LAB,
                vec![stage(&[(4, 10), (4, 11)], Finish::Step("down"))],
            );
            r.insert(PALLET_TOWN, vec![stage(PALLET_NORTH, Finish::Step("up"))]);
            r.insert(ROUTE_1, vec![stage(R1_NORTH, Finish::Step("up"))]);
        }
        "07-collect-oaks-parcel" => {
            r.insert(
                VIRIDIAN_CITY,
                vec![stage(
                    &[(20, 28), (19, 28), (19, 20), (29, 20), (29, 19)],
                    Finish::Arrive,
                )],
            );
        }
        "08-carry-it-back-to-pallet" => {
            r.insert(
                VIRIDIAN_MART,
                vec![stage(&[(3, 6), (3, 7)], Finish::Step("down"))],
            );
            // **Turn west on the row the mart door opens onto, not six rows
            // down it.** `(29,26)` is one tile above a one-way ledge —
            // measured, walking down from `(29,26)` lands on `(29,28)`, a
            // two-tile jump over row 27, and stepping up from row 28 goes
            // nowhere — so a `goto` that overshoots it is over the ledge and
            // can never get back, and the leg then spends its whole budget
            // sliding along row 28. That was 17 of 600
            // attempts, every one of them there, and it is the same shape as
            // link 04's `(4,4)` and link 05's row 4 — **a waypoint whose
            // overshoot is unrecoverable is a waypoint pointing at a trap.**
            //
            // Row 20 crosses the city cleanly (checked, x = 29 to 19), it is
            // six rows above the ledge, and the rest of the chain finishes even
            // from *below* the ledge — so the leg now survives the overshoot
            // instead of being ended by it.
            r.insert(
                VIRIDIAN_CITY,
                vec![stage(
                    &[(19, 20), (19, 30), (20, 30), (20, 35)],
                    Finish::Step("down"),
                )],
            );
            r.insert(ROUTE_1, vec![stage(R1_SOUTH, Finish::Step("down"))]);
        }
        "09-receive-the-pokedex" => {
            r.insert(PALLET_TOWN, vec![stage(PALLET_TO_LAB, Finish::Arrive)]);
            r.insert(OAKS_LAB, vec![stage(&[(4, 3), (5, 3)], Finish::Talk("up"))]);
        }
        // Back up Route 1, fighting on the way: this is the only training in
        // the chain, and a level 5 starter does not beat a bug catcher.
        "10-back-to-viridian" => {
            r.insert(
                OAKS_LAB,
                vec![stage(&[(4, 10), (4, 11)], Finish::Step("down"))],
            );
            r.insert(PALLET_TOWN, vec![stage(PALLET_NORTH, Finish::Step("up"))]);
            r.insert(ROUTE_1, vec![stage(R1_NORTH, Finish::Step("up"))]);
        }
        // **A remedial link, and its number came out of the cartridge.**
        // Viridian Forest's three bug catchers are unavoidable — pathing the
        // forest with their sight cones blocked at four, five and six tiles
        // gives no route at any of them — and the ROM's own
        // `BugCatcherData` rosters for the forest are level 6 (Weedle,
        // Caterpie), level 7 (Weedle, Kakuna, Weedle) and level 9 (Weedle):
        // six Pokémon, top level 9, and nowhere to heal in between. A lead at
        // level 6 does not survive that, which is what the runs measured. So
        // the precondition is a lead above their top level, and this link
        // exists to satisfy it.
        "11-train-for-the-forest" => {
            r.insert(
                VIRIDIAN_CITY,
                vec![stage(&[(20, 30), (20, 35)], Finish::Step("down"))],
            );
            r.insert(
                ROUTE_1,
                vec![stage(
                    &[(10, 4), (14, 4), (14, 7)],
                    Finish::Grind((11, 6), (17, 9)),
                )],
            );
        }
        "12-heal-and-climb-route-2" => {
            r.insert(
                ROUTE_1,
                vec![stage(&[(14, 2), (11, 2), (11, 0)], Finish::Step("up"))],
            );
            // Viridian City twice: into the Pokémon Center to heal whatever
            // the training cost, then out of the top of the map.
            r.insert(
                VIRIDIAN_CITY,
                vec![
                    stage(
                        &[(20, 28), (19, 28), (19, 26), (23, 26), (23, 25)],
                        Finish::Arrive,
                    ),
                    stage(
                        &[(21, 26), (21, 18), (19, 18), (19, 2), (18, 2), (18, 0)],
                        Finish::Step("up"),
                    ),
                ],
            );
            r.insert(VIRIDIAN_POKECENTER, Stage2::to_vec(POKECENTER));
            r.insert(ROUTE_2, vec![stage(R2_TO_FOREST, Finish::Arrive)]);
            r.insert(
                FOREST_SOUTH_GATE,
                vec![stage(&[(5, 1)], Finish::Step("up"))],
            );
        }
        "13-cross-viridian-forest" => {
            r.insert(
                FOREST_SOUTH_GATE,
                vec![stage(&[(5, 1)], Finish::Step("up"))],
            );
            r.insert(VIRIDIAN_FOREST, vec![stage(FOREST, Finish::Step("up"))]);
        }
        // ---- the gym arc ------------------------------------------------
        //
        // Every waypoint below is a turning point of a breadth-first path over
        // the same walkability grid the rest of the chain was drawn on, with
        // the map's own sprites treated as solid — which matters here, because
        // the gym's junior trainer stands *in* the x=3 column and a grid that
        // ignores him offers a route through his tile.
        // ---- experiments, not links ----------------------------------------
        //
        // **`scenarios/experiments/` is deliberately not `scenarios/pewter/`**:
        // `chain_files` reads only the latter, so nothing here is ever part of
        // the chain or of a certification. They exist so that a claim about
        // *where* to train is a measurement somebody else can re-run, rather
        // than a table of distances and a plausible argument.
        // `docs/agent/brock.md` has the numbers.
        "x-train-route-2-south" => {
            r.insert(
                VIRIDIAN_CITY,
                vec![stage(
                    &[(20, 28), (19, 28), (19, 2), (18, 2), (18, 0)],
                    Finish::Step("up"),
                )],
            );
            r.insert(
                ROUTE_2,
                vec![stage(
                    &[
                        (8, 62),
                        (7, 62),
                        (7, 57),
                        (5, 57),
                        (5, 56),
                        (4, 56),
                        (4, 50),
                    ],
                    Finish::Grind((4, 48), (8, 51)),
                )],
            );
        }
        "16-heal-in-pewter" => {
            // Pewter's Pokemon Center is the same building as Viridian's:
            // nurse at (3,1) behind a counter, (3,2) solid, and the place to
            // stand is (3,3) facing up. Checked against the cartridge rather
            // than assumed — both maps use tileset 6 and their grids agree
            // tile for tile in that corner.
            // **From wherever link 15 stopped**, which is in the grass on
            // Route 2 rather than in Pewter: a training link's exit state is
            // taken the instant the level ticks over, and that is wherever the
            // fight that earned it happened. The same shape link 12 has, for
            // the same reason — link 11 leaves the player on Route 1.
            r.insert(ROUTE_2, vec![stage(ROUTE_2_TO_PEWTER, Finish::Step("up"))]);
            r.insert(
                PEWTER_CITY,
                vec![stage(&[(18, 26), (13, 26), (13, 25)], Finish::Arrive)],
            );
            r.insert(PEWTER_POKECENTER, Stage2::to_vec(POKECENTER));
        }
        // **A repair, and the second one this chain has needed.** Link 11
        // exists because Viridian Forest's bug catchers beat a level 6 lead;
        // this exists because Brock beats a level 10 one, and for two of the
        // three starters it is not close. Measured, 40 attempts per starter
        // from the certified pool: SQUIRTLE 90.0%, CHARMANDER 20.0%,
        // BULBASAUR 0.0%. The cause is a move, not a number — see
        // `docs/agent/brock.md`.
        "15-train-for-brock" => {
            r.insert(
                PEWTER_CITY,
                vec![stage(PEWTER_TO_ROUTE_2, Finish::Step("down"))],
            );
            r.insert(
                ROUTE_2,
                vec![stage(ROUTE_2_TO_GRASS, Finish::Grind((1, 3), (6, 6)))],
            );
        }
        // **The one link in the chain that has to WIN a fight.** Every battle
        // before this had a goal the cartridge sets whoever won — the rival
        // battle's flag is set by `OaksLabRivalEndBattleScript` after a loss
        // as much as after a win — so a passing rate measured surviving
        // rather than battling. A badge bit only sets if Brock is beaten.
        "17-fight-through-the-gym" => {
            r.insert(PEWTER_POKECENTER, Stage2::to_vec(LEAVE_POKECENTER));
            r.insert(PEWTER_CITY, vec![stage(PEWTER_TO_GYM, Finish::Arrive)]);
            r.insert(PEWTER_GYM, vec![stage(GYM_TO_BROCK, Finish::Talk("up"))]);
        }
        "18-heal-after-the-gym" => {
            r.insert(
                PEWTER_GYM,
                vec![stage(&[(4, 12), (4, 13)], Finish::Step("down"))],
            );
            r.insert(PEWTER_CITY, vec![stage(GYM_TO_POKECENTER, Finish::Arrive)]);
            r.insert(PEWTER_POKECENTER, Stage2::to_vec(POKECENTER));
        }
        // East out of Pewter onto Route 3, by a map connection — the same
        // shape as link 14's arrival and for the same reason: Pewter's
        // connection byte is 0b0101, and reading each block's own destination
        // gives south -> Route 2 and east -> Route 3.
        "19-east-to-route-3" => {
            r.insert(PEWTER_POKECENTER, Stage2::to_vec(LEAVE_POKECENTER));
            r.insert(
                PEWTER_CITY,
                vec![stage(PEWTER_TO_ROUTE_3, Finish::Step("right"))],
            );
        }
        "14-arrive-in-pewter" => {
            r.insert(
                FOREST_NORTH_GATE,
                vec![stage(&[(5, 1)], Finish::Step("up"))],
            );
            r.insert(
                ROUTE_2,
                vec![stage(
                    &[(3, 11), (3, 2), (8, 2), (8, 0)],
                    Finish::Step("up"),
                )],
            );
        }
        _ => {}
    }
    r
}

/// The detour a leg takes when the party is too hurt to go on.
///
/// **A remedial link needs this and an ordinary one does not.** Route 1's
/// grass is the only experience available before the forest, and a lead below
/// half health runs from every encounter — so without somewhere to heal, a
/// "train to level 10" link runs from everything for ever and gains nothing.
/// Measured: 2,500 decisions parked at 5/21 HP without a single fight.
///
/// The Pokémon Center is free and full and costs the walk; a Potion is instant
/// and consumes the bag. This takes the Center because the walk is four map
/// transitions the solver already knows and the bag is empty — and that is a
/// choice worth stating rather than a default.
fn heal_route(scenario: &str) -> HashMap<u8, MapPlan> {
    let mut r: HashMap<u8, MapPlan> = HashMap::new();
    // **The gym link needs one and every other link in Pewter does not.**
    // Its two trainers are back to back with nothing between them, and the
    // nurse is a five-minute walk away across the city — so a lead that beats
    // the junior trainer and then walks straight into Brock arrives at the
    // fight that matters already hurt. Measured in `docs/agent/brock.md`:
    // fighting both without healing between costs a level of effective
    // strength.
    // Training on Route 2's grass, healing in Pewter — a dozen tiles each way.
    // The Viridian nurse, from Route 2's southern grass: down the map, into
    // the city, across it. Fifty-six steps against Route 1's twenty-five.
    if scenario == "x-train-route-2-south" {
        r.insert(
            ROUTE_2,
            vec![stage(
                &[
                    (4, 56),
                    (5, 56),
                    (5, 57),
                    (7, 57),
                    (7, 62),
                    (8, 62),
                    (8, 71),
                ],
                Finish::Step("down"),
            )],
        );
        r.insert(
            VIRIDIAN_CITY,
            vec![stage(
                &[(18, 2), (19, 2), (19, 26), (23, 26), (23, 25)],
                Finish::Arrive,
            )],
        );
        r.insert(VIRIDIAN_POKECENTER, Stage2::to_vec(POKECENTER));
    }
    if scenario == "15-train-for-brock" {
        r.insert(ROUTE_2, vec![stage(ROUTE_2_TO_PEWTER, Finish::Step("up"))]);
        r.insert(
            PEWTER_CITY,
            vec![stage(
                &[(18, 31), (13, 31), (13, 26), (13, 25)],
                Finish::Arrive,
            )],
        );
        r.insert(PEWTER_POKECENTER, Stage2::to_vec(POKECENTER));
    }
    if scenario == "17-fight-through-the-gym" {
        r.insert(
            PEWTER_GYM,
            vec![stage(&[(4, 12), (4, 13)], Finish::Step("down"))],
        );
        r.insert(PEWTER_CITY, vec![stage(GYM_TO_POKECENTER, Finish::Arrive)]);
        r.insert(PEWTER_POKECENTER, Stage2::to_vec(POKECENTER));
    }
    if scenario == "11-train-for-the-forest" {
        r.insert(
            ROUTE_1,
            vec![stage(&[(14, 2), (11, 2), (11, 0)], Finish::Step("up"))],
        );
        r.insert(
            VIRIDIAN_CITY,
            vec![stage(
                &[(20, 28), (19, 28), (19, 26), (23, 26), (23, 25)],
                Finish::Arrive,
            )],
        );
        r.insert(VIRIDIAN_POKECENTER, Stage2::to_vec(POKECENTER));
    }
    r
}

/// Should this leg's solver fight wild Pokémon, or run from them?
///
/// **A `battle flee` rule forfeits a run that won a battle**, so this must
/// agree with the scenario file. The legs that allow fighting are the ones
/// where the party has to grow: nothing before Viridian Forest earns any
/// experience at all, and a level 5 starter does not beat a bug catcher.
fn may_fight_wild(scenario: &str) -> bool {
    // **Not in the forest**, and that is a measured choice rather than a
    // squeamish one. Route 1 and Route 2 are Pidgey and Rattata, which are
    // worth beating; Viridian Forest is Caterpie, Weedle and their cocoons,
    // which give almost no experience and cost about fifteen decisions each
    // to fight against four to run from. Measured: fighting them spent the
    // whole 2,500-call budget without leaving the maze, and left the party
    // worn down for the bug catcher who cannot be run from.
    // **And not on the walk back to Viridian either.** That leg's job is to
    // arrive; training is link 11's, which exists for exactly that. Fighting
    // there put the lead in a fight it could not finish once in 1,500 — the
    // escape fails, Gen 1 lets it fail again, and no policy can rescue a
    // Pokémon that is already one hit from dead with no Pokémon Center on the
    // leg. **The cost is real and belongs beside the benefit**: the lead
    // arrives at link 11 a level or so lower, and link 11 has to make it up.
    matches!(
        scenario,
        "05-the-rival-battle"
            | "11-train-for-the-forest"
            | "15-train-for-brock"
            | "x-train-route-2-south"
    )
}

/// The level the solver trains its lead to before it stops picking fights.
///
/// **This must agree with the scenario's own goal**, and the hazard is the same
/// one [`may_fight_wild`] carries: two places saying the same thing. It is a
/// function rather than a constant because the two remedial links aim at
/// different numbers, and those numbers came from different parts of the
/// cartridge — link 11's from Viridian Forest's bug-catcher rosters, link 15b's
/// from which level each starter's line first holds a move that answers
/// ROCK/GROUND.
///
/// `$GB_TRAIN_TO` overrides it, for sweeping the threshold; a typo is ignored
/// rather than silently changing the target.
fn train_to(scenario: &str) -> u32 {
    if let Some(v) = std::env::var("GB_TRAIN_TO")
        .ok()
        .and_then(|v| v.parse().ok())
    {
        return v;
    }
    match scenario {
        "11-train-for-the-forest" => 13,
        "15-train-for-brock" => BROCK_TRAIN_TO,
        "x-train-route-2-south" => 13,
        _ => 13,
    }
}

/// The level [`train_to`] aims at before the gym. See `docs/agent/brock.md`.
const BROCK_TRAIN_TO: u32 = 16;

/// Where each leg is trying to get to, so an off-route map can be recognised
/// as off-route rather than merely unknown.
fn on_route(scenario: &str) -> Vec<u8> {
    route_for(scenario, 0).keys().copied().collect()
}

// ---------------------------------------------------------------------------
// the snapshot, as the driver sees it
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Snap {
    in_game: bool,
    loaded: bool,
    map: u8,
    x: u8,
    y: u8,
    party: usize,
    party_hp: u32,
    lead_level: u32,
    lead_hp: u32,
    lead_max_hp: u32,
    battle: Option<String>,
    /// The battle facts the shared policy decides from. `None` outside a
    /// battle, and also inside one whose structs the engine has not filled yet.
    arena: Option<terminalgb_plugin_agent::battle::View>,
    kind: String,
    trust: String,
    text: String,
    selected: String,
    waiting: bool,
    cursor_row: i64,
    /// `(x, y, to_map)` of every door the observer can see on this map.
    exits: Vec<(u8, u8, u8)>,
    last: Vec<String>,
}

impl Snap {
    fn parse(request: &str) -> Snap {
        let mut s = Snap::default();
        let Ok(v) = json::parse(request) else {
            return s;
        };
        if let Some(g) = v.get("game") {
            s.in_game = g.get("in_game").and_then(|b| b.as_bool()).unwrap_or(false);
            if let Some(m) = g.get("map") {
                s.map = m.u64_field("id").unwrap_or(0) as u8;
                s.x = m.u64_field("x").unwrap_or(0) as u8;
                s.y = m.u64_field("y").unwrap_or(0) as u8;
                // The observer sends `loaded:false` instead of a size when no
                // map header is loaded, which is the real "am I in control?"
                // test — `wCurMap` reads the bedroom all through the intro.
                s.loaded = m.u64_field("width").is_some();
            }
            s.party = g
                .get("party")
                .and_then(|p| p.as_array())
                .map(|a| a.len())
                .unwrap_or(0);
            s.party_hp = g
                .get("party")
                .and_then(|p| p.as_array())
                .map(|a| a.iter().filter_map(|m| m.u64_field("hp")).sum::<u64>() as u32)
                .unwrap_or(0);
            if let Some(lead) = g
                .get("party")
                .and_then(|p| p.as_array())
                .and_then(|a| a.first())
            {
                s.lead_level = lead.u64_field("level").unwrap_or(0) as u32;
                s.lead_hp = lead.u64_field("hp").unwrap_or(0) as u32;
                s.lead_max_hp = lead.u64_field("max_hp").unwrap_or(1) as u32;
            }
            if let Some(b) = g.get("battle") {
                s.battle = Some(b.str_field("kind").unwrap_or("trainer").to_string());
                s.arena = terminalgb_plugin_agent::battle::View::parse(b);
            }
            if let Some(e) = g.get("exits").and_then(|e| e.as_array()) {
                for x in e {
                    s.exits.push((
                        x.u64_field("x").unwrap_or(0) as u8,
                        x.u64_field("y").unwrap_or(0) as u8,
                        x.u64_field("to_map").unwrap_or(255) as u8,
                    ));
                }
            }
        }
        if let Some(sc) = v.get("screen") {
            s.kind = sc.str_field("kind").unwrap_or("").to_string();
            s.trust = sc.str_field("trust").unwrap_or("").to_string();
            s.text = sc.str_field("text").unwrap_or("").to_string();
            s.selected = sc.str_field("selected").unwrap_or("").to_string();
            s.waiting = sc
                .get("waiting_for_input")
                .and_then(|b| b.as_bool())
                .unwrap_or(false);
            s.cursor_row = sc
                .get("menu_cursor_row")
                .and_then(|r| r.as_u64())
                .map(|r| r as i64)
                .unwrap_or(-1);
        }
        if let Some(a) = v.get("last_actions").and_then(|a| a.as_array()) {
            for o in a {
                s.last.push(o.str_field("result").unwrap_or("").to_string());
            }
        }
        s
    }

    fn said(&self, needle: &str) -> bool {
        self.text.to_ascii_uppercase().contains(needle)
    }

    fn stuck_last(&self) -> bool {
        self.last.iter().any(|r| r.starts_with("stuck at"))
    }
}

// ---------------------------------------------------------------------------
// the driver
// ---------------------------------------------------------------------------

/// A plan, as JSON, built one action at a time.
struct Plan {
    say: String,
    acts: Vec<String>,
}

impl Plan {
    fn new(say: &str) -> Plan {
        Plan {
            say: say.to_string(),
            acts: Vec::new(),
        }
    }
    fn press(mut self, b: &str, times: u32) -> Plan {
        self.acts.push(format!(
            "{{\"do\":\"press\",\"button\":\"{b}\",\"times\":{times}}}"
        ));
        self
    }
    fn walk(mut self, d: &str, steps: u32) -> Plan {
        self.acts.push(format!(
            "{{\"do\":\"walk\",\"dir\":\"{d}\",\"steps\":{steps}}}"
        ));
        self
    }
    fn goto(mut self, x: u8, y: u8, max_steps: u32) -> Plan {
        self.acts.push(format!(
            "{{\"do\":\"goto\",\"x\":{x},\"y\":{y},\"max_steps\":{max_steps}}}"
        ));
        self
    }
    fn text(mut self, n: u32) -> Plan {
        self.acts
            .push(format!("{{\"do\":\"advance_text\",\"max_presses\":{n}}}"));
        self
    }
    fn wait(mut self, frames: u32) -> Plan {
        self.acts
            .push(format!("{{\"do\":\"wait\",\"frames\":{frames}}}"));
        self
    }
    fn finish(self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("say", &self.say);
        w.end_obj();
        // Hand-assembled rather than through the writer, because the plan is a
        // list of pre-rendered objects and the writer has no raw-item escape.
        let head = w.finish();
        let head = head.trim_end_matches('}');
        format!("{head},\"plan\":[{}]}}", self.acts.join(","))
    }
}

pub struct ChainDriver {
    route: HashMap<u8, MapPlan>,
    on_route: Vec<u8>,
    rng: u64,
    /// Which waypoint on the current stage is next.
    idx: usize,
    /// Every waypoint on the current stage has been reached.
    completed: bool,
    /// Per map: which stage of its plan is current. See [`MapPlan`].
    stage: HashMap<u8, usize>,
    at_map: Option<u8>,
    /// Consecutive decisions on the same map with no waypoint progress.
    barren: u32,
    /// Consecutive battle turns, so a fight that will not end is escaped.
    battle_turns: u32,
    /// The action the shared policy chose for the turn now being worked
    /// through the menus. Held so the walk from the main menu to the move list
    /// cannot change its mind halfway and leave the cursor somewhere odd.
    plan_this_turn: Option<terminalgb_plugin_agent::battle::Action>,
    /// Battle telemetry, printed with the run's verdict — a link can pass while
    /// the battle policy is quietly awful.
    tally: BattleTally,
    /// One primitive button per decision instead of a compound plan. See
    /// [`ChainDriver::primitive`].
    primitive: bool,
    /// Where every decision is written, one JSON line each: the observation
    /// the driver saw and the action it chose.
    trail: Option<std::sync::Arc<std::sync::Mutex<std::fs::File>>>,
    /// Consecutive decisions spent pressing through the same unchanged screen.
    text_tries: u32,
    last_text: String,
    /// The name menu, held across the decisions it takes to work it.
    ///
    /// `row0` is the **screen row** the cursor sat on when the menu first
    /// appeared, i.e. NEW NAME's row; `want` is how far down the preset list
    /// this attempt is aiming. Both are needed because the observer reports a
    /// screen row and not a menu index, and subtracting one from the other is
    /// what silently made every attempt pick NEW NAME.
    name_row0: Option<u8>,
    name_want: u8,
    /// Whether this leg's rules allow winning a battle at all.
    fight_wild: bool,
    /// Where to go when the party is too hurt to keep training, and whether
    /// that is what is happening right now.
    heal_route: HashMap<u8, MapPlan>,
    healing: bool,
    /// Set when the battle policy declined a fight because the lead is too
    /// hurt, and cleared when the walk to the nurse begins. **The one place
    /// the solver learns the fight threshold rather than guessing at it.**
    refused_a_fight: bool,
    /// The level the solver trains its lead up to before it stops picking
    /// fights. Nothing before Viridian Forest earns any experience, and a
    /// level 5 starter does not beat a bug catcher.
    train_to: u32,
    pending: Option<String>,
    busy: bool,
    calls: u32,
}

impl ChainDriver {
    pub fn new(scenario: &str, seed: u64) -> ChainDriver {
        ChainDriver {
            route: route_for(scenario, seed),
            on_route: on_route(scenario),
            // **Not `seed | 1`.** That is the idiom `RandomDriver` uses to
            // avoid a zero state, and it maps every even seed onto its odd
            // neighbour — so half the corpus is a duplicate of the other half.
            // Measured: seeds 0 and 1 produced byte-identical recordings, as
            // did 2 and 3. Mix first, then force the low bit.
            rng: seed
                .wrapping_add(0x9E37_79B9_7F4A_7C15)
                .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                | 1,
            idx: 0,
            completed: false,
            stage: HashMap::new(),
            at_map: None,
            barren: 0,
            battle_turns: 0,
            plan_this_turn: None,
            tally: BattleTally::default(),
            primitive: false,
            trail: None,
            text_tries: 0,
            last_text: String::new(),
            name_row0: None,
            name_want: 0,
            fight_wild: may_fight_wild(scenario),
            heal_route: heal_route(scenario),
            healing: false,
            refused_a_fight: false,
            train_to: train_to(scenario),
            pending: None,
            busy: false,
            calls: 0,
        }
    }

    /// **One button per decision, and nothing compound.**
    ///
    /// The compound plans (`goto` a waypoint, `advance_text` through a
    /// conversation) are what make the expert fast, and they are exactly what
    /// makes its trajectories useless as demonstrations: `goto (19,26)` is not
    /// an action a policy without the route table could ever emit. In
    /// primitive mode every decision is one of nine things a player can do —
    /// the eight buttons plus "press through the text" — so an
    /// observation/action pair is a pair somebody could learn from and a
    /// student can be evaluated on the same footing.
    ///
    /// It works because a waypoint chain is the *turning points* of a
    /// breadth-first path, and the runs between turning points are axis
    /// aligned. Stepping toward the next waypoint on whichever axis still has
    /// distance left therefore walks the same path the `goto` would have,
    /// one tile at a time.
    pub fn primitive(mut self, on: bool) -> Self {
        self.primitive = on;
        self
    }

    /// Write one JSON line per decision to `path`.
    pub fn trailing_to(
        mut self,
        f: std::sync::Arc<std::sync::Mutex<std::fs::File>>,
    ) -> Self {
        self.trail = Some(f);
        self
    }

    fn next(&mut self) -> u64 {
        // xorshift64*, so a run is reproducible from its seed.
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn roll(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// The one action this decision is, in primitive mode. Everything that
    /// would have been a compound plan collapses to its first button.
    fn one(&self, say: &str, button: &str) -> String {
        Plan::new(say).press(button, 1).finish()
    }

    fn decide(&mut self, s: &Snap) -> String {
        // **Watch the battle on every decision, not only on the ones that open
        // a menu.** The knock-out happens during the text that follows a turn,
        // so a tally fed only from the menu branch never sees a party at 0 HP
        // and scores every battle a win. Measured: it reported 300 of 300 won
        // where the exit states said 218.
        if let Some(a) = &s.arena {
            self.tally.observe(a);
        }
        // Score a battle the moment it is over, from the last state it was in.
        self.tally.tick(s.battle.is_some());
        // 1. Not in the game yet, or the map header is not loaded: this is the
        //    introduction, a warp or a cutscene, and the only useful verbs are
        //    menu ones.
        if !s.in_game || !s.loaded {
            return self.intro(s);
        }
        // 2. A battle owns the screen until it is over.
        if s.battle.is_some() {
            return self.fight(s);
        }
        self.battle_turns = 0;
        // 3. A menu or a conversation. `advance_text` refuses to press A on the
        //    naming keyboard and says so, which is why that case is first.
        if s.kind == "naming-keyboard" {
            return Plan::new("Confirm the name with START.")
                .press("start", 1)
                .finish();
        }
        if s.kind == "yes-no" {
            return self.yes_no(s);
        }
        // The start menu, or the options screen behind it. Nothing in this
        // chain wants either, and A in a menu chooses something — so the only
        // safe answer is B, which is the one button that can never open a menu.
        if s.said("OPTION") && (s.said("EXIT") || s.said("TEXT SPEED")) {
            if self.primitive {
                return self.one("Close the menu.", "b");
            }
            return Plan::new("Close the menu.").press("b", 2).finish();
        }
        // **`text` blocks movement exactly as `dialogue` does, and only the
        // kind says so.** The Pokémon Center nurse's box carries no `▼`, so
        // the observer classifies it `text` with `waiting_for_input` false —
        // and a solver that treats "nothing is asking for a button" as "I may
        // walk" issues 800 walk plans into a box that swallows every one of
        // them. Measured: link 10 spent its entire call budget at (3,3) with
        // "OK. We'll need your POKéMON." on screen. The overworld reports
        // `no-text`, so pressing through `text` costs nothing where it is not
        // needed.
        if s.waiting || matches!(s.kind.as_str(), "dialogue" | "menu" | "text") {
            if s.text == self.last_text {
                self.text_tries += 1;
            } else {
                self.text_tries = 0;
                self.last_text = s.text.clone();
            }
            // **And it has to be bounded, or the cure is the same deadlock.**
            // Gen 1 redraws the tilemap when the player *moves*, so a box the
            // script has already finished with stays on screen until you walk
            // — and a solver that answers "there is text" with "press A" then
            // never walks, so the text never clears. Measured at the Pokémon
            // Center nurse: healed, done, and 850 decisions of A into "OK.
            // We'll need your POKéMON." Three tries at an unchanged screen and
            // then walk anyway, which clears it.
            if self.text_tries < 3 {
                return Plan::new("Read what the game is saying.").text(24).finish();
            }
        } else {
            self.text_tries = 0;
        }
        // 4. The overworld. Before routing, decide whether this is a walk to
        //    the goal or a walk to the nurse: below a third of full health the
        //    lead runs from everything, so training cannot resume until it is
        //    healed.
        //
        //    **The heal threshold has to be the fight threshold**, or the
        //    policy has a band where it does neither. Measured twice, two
        //    years of this rule apart in spirit: with healing at a third and
        //    fighting at a half the lead settled at 8 of 23 HP and ran from
        //    everything for 2,402 decisions; and then, once the shared policy
        //    learned to weigh a critical hit, at 12 of 23 — above the flat
        //    half, so no heal — where it refused 94 of 100 encounters. So the
        //    trigger is no longer a number here at all. `refused_a_fight` is
        //    set by the policy itself, in `fight`, when it declines because
        //    the lead is too hurt; the flat half stays only as a floor for the
        //    case where the lead is hurt and nothing has attacked it yet.
        if !self.heal_route.is_empty() && s.lead_max_hp > 0 {
            let hurt = self.refused_a_fight || s.lead_hp * 2 <= s.lead_max_hp;
            let full = s.lead_hp >= s.lead_max_hp;
            if !self.healing && hurt {
                self.healing = true;
                self.refused_a_fight = false;
                self.stage.clear();
                self.idx = 0;
                self.at_map = None;
            } else if self.healing && full {
                self.healing = false;
                self.stage.clear();
                self.idx = 0;
                self.at_map = None;
            }
        }
        self.travel(s)
    }

    /// The title screen, NEW GAME, Oak's speech and the two name menus.
    fn intro(&mut self, s: &Snap) -> String {
        // The name menus are the one place where mashing A is actively wrong:
        // the cursor starts on NEW NAME, which opens the keyboard, and on the
        // keyboard A types a letter for ever.
        if s.said("NEW NAME") {
            // Presets sit below NEW NAME; picking one skips the keyboard
            // entirely, and which one is a free axis of diversity.
            //
            // **One key per decision, off the cursor's own row.** Two things
            // were wrong here and each on its own made every attempt pick NEW
            // NAME — which opens the keyboard, so the name became whatever the
            // burst typed, identically, for every seed. First, `menu_cursor_row`
            // is the **screen row** the `▶` is drawn on, not the menu item's
            // index, so subtracting it from an item index compared two
            // different quantities: on the rival's menu it gave zero and A fell
            // on NEW NAME. Second, `down` and `a` in one plan is a burst in a
            // Gen 1 menu, and `HandleMenuInput` delays several frames after
            // accepting a press, so the first of a burst is routinely swallowed
            // and A lands one item early. Anchoring on the row the menu opened
            // at and pressing one key per decision is immune to both: a
            // swallowed press leaves the row where it was and simply repeats.
            let row = s.cursor_row.max(0) as u8;
            let row0 = match self.name_row0 {
                Some(r) => r,
                None => {
                    self.name_want = 1 + self.roll(3) as u8;
                    self.name_row0 = Some(row);
                    row
                }
            };
            if row.saturating_sub(row0) < self.name_want {
                return self.one("Down the name list.", "down");
            }
            return self.one("Take this preset name.", "a");
        }
        // Off the menu: the next one gets its own target and its own anchor.
        self.name_row0 = None;
        if s.kind == "naming-keyboard" {
            return Plan::new("Confirm with START.").press("start", 1).finish();
        }
        if s.said("NEW GAME") {
            // NEW GAME is the first item on a fresh cartridge; on a cartridge
            // with a save it is the second. Aim at the row that says it.
            let up = s.cursor_row.max(0) as u32;
            if self.primitive {
                return if up > 0 {
                    self.one("Up the menu towards NEW GAME.", "up")
                } else {
                    self.one("Start a new game.", "a")
                };
            }
            let p = Plan::new("Start a new game.");
            let p = if up > 0 { p.press("up", up) } else { p };
            return p.press("a", 1).finish();
        }
        // **A only.** START is what gets past the title screen, and A does
        // that too — but a START that lands after the game has begun opens the
        // start menu, and a state captured with it held carries the menu into
        // the *next* link. Measured: link 2 booted into OPTION and spent its
        // whole budget changing the text speed.
        if self.primitive {
            return self.one("Press on through the introduction.", "a");
        }
        Plan::new("Press on through the introduction.")
            .press("a", 8)
            .finish()
    }

    fn yes_no(&mut self, s: &Snap) -> String {
        // Every yes/no on this chain wants YES: taking a starter, and the
        // nurse. The cursor starts on YES, so A is the answer.
        let _ = s;
        if self.primitive {
            return self.one("Say yes.", "a");
        }
        Plan::new("Say yes.").press("a", 1).text(8).finish()
    }

    /// **The battle policy is not here.** It is
    /// [`terminalgb_plugin_agent::battle`], one implementation shared by every
    /// link that meets a battle, deciding fight/item/switch/run from the
    /// cartridge's own state — the type chart and move table read out of the
    /// ROM, both sides' HP and stats out of the two battle structs, the bag out
    /// of `wBagItems`. What is here is the *menus*: turning that decision into
    /// keypresses without going out of phase.
    fn fight(&mut self, s: &Snap) -> String {
        use terminalgb_plugin_agent::battle::Action;
        self.battle_turns += 1;
        let sel = s.selected.to_ascii_uppercase();
        if s.waiting || s.kind == "dialogue" || s.kind == "text" {
            // Between turns. The next menu gets a fresh decision.
            self.plan_this_turn = None;
            if self.primitive {
                return self.one("Let the battle text finish.", "a");
            }
            return Plan::new("Let the battle text finish.").text(20).finish();
        }
        let Some(view) = s.arena.as_ref() else {
            // The engine has not filled the battle structs yet; there is
            // nothing to decide from, so let it.
            return Plan::new("Let the battle settle.").wait(16).finish();
        };
        let intent = terminalgb_plugin_agent::battle::Intent {
            may_fight_wild: self.fight_wild,
            train_to: self.train_to as u8,
        };
        // How many turns have already gone on a stat move is the driver's
        // count; the cartridge keeps none, so the view carries ours.
        let mut view = view.clone();
        view.stat_turns = self.tally.stat_turns;
        let view = &view;
        let action = self
            .plan_this_turn
            .get_or_insert_with(|| terminalgb_plugin_agent::battle::decide(view, &intent))
            .clone();

        // The party list, reached either on purpose (a switch) or by a
        // mispress. B is the one button that can only ever back out of a menu.
        if s.said("CHOOSE A POK") {
            if let Action::Switch { to, why } = action {
                return self.menu_to(view.menu_item, to as u8, why, None);
            }
            return self.one("Back out of the party list.", "b");
        }
        if s.kind != "menu" {
            if self.primitive {
                return self.one("Let the battle run on.", "a");
            }
            return Plan::new("Let the battle run on.").text(20).finish();
        }

        // **Navigate off the cursor, one key per decision.** Gen 1's
        // `HandleMenuInput` delays several frames after accepting a press, so
        // the first of a burst is routinely swallowed and the rest land out of
        // phase. Measured: a run policy that fired down/right/A blind opened
        // the party list instead, 2,500 decisions of it. A swallowed press
        // simply repeats when each decision is one key.
        let on_main = s.said("FIGHT");
        if !on_main {
            // The move list. **Its cursor is 1-based and the main menu's is
            // not**, which the cartridge says itself: `MoveSelectionMenu`
            // (`engine/battle/core.asm`) opens with
            // `ld a, [wPlayerMoveListIndex] / inc a / ld [hli], a ;
            // wCurrentMenuItem`. Measured before it was believed — the cursor
            // reads 1 with `▶` drawn on the first move.
            if let Action::Fight { slot, why } = action {
                return self.menu_to(
                    view.menu_item.saturating_sub(1),
                    slot as u8,
                    why,
                    Some(slot),
                );
            }
            // Some other list we did not ask for.
            return self.one("Back out of this list.", "b");
        }
        // The main menu: FIGHT / PKMN over ITEM / RUN.
        let want = match &action {
            Action::Fight { .. } => "FIGHT",
            Action::Item { .. } => "ITEM",
            Action::Run { .. } => "RUN",
            Action::Switch { .. } => "PKMN",
        };
        if sel.contains(want) {
            // **Committed.** Everything above this line is navigation, and
            // counting a decision to run or a stat move there counts the
            // keypresses that walked to it: measured, 1,048 wild battles
            // reported 3,158 runs and 3,158 failed escapes, all of them one
            // battle's `down`, `right`, `a`. Worse, the count feeds the
            // policy's own cap on stat moves, so `MAX_STAT_TURNS` of two was
            // really about one.
            if !matches!(action, Action::Fight { .. }) {
                self.tally.note(action.why(), None);
            }
            // **The heal threshold IS the fight threshold, and this is where
            // the two are made one thing rather than two numbers that agree
            // for a while.** The policy's "too hurt" is threat-relative — it
            // weighs our HP against what the thing in front of us hits for on
            // a critical — and nothing outside a battle can compute it, so a
            // solver that re-derives a flat fraction re-opens the dead band
            // every time the policy gets cleverer. Instead the refusal itself
            // is the signal: if the policy just declined a fight because the
            // lead is too hurt, the lead is too hurt, and the next overworld
            // decision walks to the nurse.
            if action.why() == terminalgb_plugin_agent::battle::why::TOO_HURT {
                self.refused_a_fight = true;
            }
            return self.one(action.why(), "a");
        }
        // FIGHT PKMN
        // ITEM  RUN
        let here = ["FIGHT", "PKMN", "ITEM", "RUN"]
            .iter()
            .position(|w| sel.contains(w))
            .unwrap_or(0);
        let there = ["FIGHT", "PKMN", "ITEM", "RUN"]
            .iter()
            .position(|w| *w == want)
            .unwrap_or(0);
        let key = if here / 2 != there / 2 {
            if there / 2 > here / 2 {
                "down"
            } else {
                "up"
            }
        } else if there % 2 > here % 2 {
            "right"
        } else {
            "left"
        };
        self.one(&format!("towards {want}"), key)
    }

    /// Step a menu cursor one row towards `want`, or press A when it is there.
    ///
    /// `wCurrentMenuItem` is the live index, so this cannot drift: a swallowed
    /// press leaves the index where it was and the next decision repeats it.
    fn menu_to(&mut self, at: u8, want: u8, why: &str, chose: Option<usize>) -> String {
        let say = format!("{why} (slot {})", want + 1);
        match at.cmp(&want) {
            std::cmp::Ordering::Less => self.one(&say, "down"),
            std::cmp::Ordering::Greater => self.one(&say, "up"),
            // Committed: this is the press that spends the turn, and the only
            // place a turn may be counted.
            std::cmp::Ordering::Equal => {
                self.tally.note(why, chose);
                self.one(&say, "a")
            }
        }
    }

    /// Follow the route on the current map.
    fn travel(&mut self, s: &Snap) -> String {
        if self.at_map != Some(s.map) {
            // Leaving a map having reached the *last* leg of its current
            // stage advances that map's program counter, so coming back does
            // the next thing rather than repeating the last one. Bumping into
            // the wrong building, or being pushed out by a script, does not
            // advance: an accident must never skip a leg.
            //
            // **The test is "on the last leg", not "completed".** A stage that
            // ends on a warp tile can never report completion, because
            // standing on the tile *is* the warp and there is no decision
            // afterwards to notice it. Measured: Viridian City's Pokémon
            // Center stage ends on (23,25), so the city's counter never moved
            // and the solver healed, walked out, and went straight back in,
            // for 469 decisions.
            if let Some(prev) = self.at_map {
                let table = if self.healing {
                    &self.heal_route
                } else {
                    &self.route
                };
                let last_leg = table
                    .get(&prev)
                    .and_then(|st| {
                        let n = *self.stage.get(&prev).unwrap_or(&0);
                        st.get(n)
                    })
                    .map(|st| self.idx + 1 >= st.via.len())
                    .unwrap_or(false);
                if last_leg {
                    let e = self.stage.entry(prev).or_insert(0);
                    *e += 1;
                }
            }
            self.at_map = Some(s.map);
            self.idx = 0;
            self.barren = 0;
            self.completed = false;
            // **The first observation after a warp is stale.** The map id
            // changes a frame before the coordinates do, so a plan built on
            // this snapshot is aimed from where the player *was*. Measured on
            // link 08: leaving the Viridian Mart reported (3,7) — the mart's
            // own coordinate — with the map already reading Viridian City, and
            // the goto that followed walked past the one gap in row 27 and
            // could not get back, 74 times in 300.
            return Plan::new("Let the new map settle.").wait(24).finish();
        }
        let table = if self.healing {
            &self.heal_route
        } else {
            &self.route
        };
        let Some(stages) = table.get(&s.map).cloned() else {
            return self.off_route(s);
        };
        let n = *self.stage.get(&s.map).unwrap_or(&0);
        // Past the end of the list: keep doing the last thing rather than
        // falling off it, because the last thing is usually "leave".
        let st = match stages.get(n).or_else(|| stages.last()) {
            Some(st) => st.clone(),
            None => return self.off_route(s),
        };

        // **A waypoint counts as reached only when it is stood on.** An
        // earlier version allowed a tile of slack, on the reasoning that a
        // `goto` stopping beside its target had arrived for practical
        // purposes. It had not, twice over. The last waypoint is the thing the
        // leg is *for* — the tile Oak's script watches, the tile a warp sits on
        // — so a tile of slack there parks the run one step short for ever
        // (measured: link 3 at (10,2) with the trigger on (10,1)). And every
        // other waypoint is a turning point of a breadth-first path, which is
        // to say the place where the way through lines up: Viridian City's row
        // 27 is `#.###.##############`, and standing one tile from (19,28) is
        // standing in front of a wall. Measured there too — 430 decisions
        // oscillating between (20,28) and (21,28).
        let before = self.idx;
        while self.idx < st.via.len() && (s.x, s.y) == st.via[self.idx] {
            self.idx += 1;
        }
        if self.idx > before {
            self.barren = 0;
        } else {
            self.barren += 1;
        }
        self.completed = self.idx >= st.via.len();

        // Stuck: the executor said walls are in the way, or several decisions
        // have bought no progress. Commit to a lateral run and try again — the
        // same escape `GoTo`'s own detour uses, one level up.
        //
        // **Only while there are waypoints left.** A map whose stage is empty
        // (the rival challenges you where you stand; Oak walks you in) makes
        // no waypoint progress by definition, so an unguarded barren counter
        // reads a scripted cutscene as being stuck and walks away from it.
        // Measured: link 5 wandered to (4,6) and lost the battle it was
        // supposed to be standing still for.
        if !self.completed && (s.stuck_last() || self.barren >= 5) {
            self.barren = 0;
            let (wx, wy) = st.via[self.idx];
            // Sidestep along the axis with the *least* left to cover: the
            // obstacle is almost always across the long axis, so committing
            // to the short one is what gets round a building.
            //
            // **And on that axis, go the way the waypoint is.** The direction
            // used to be a coin flip whichever way the waypoint lay, so half
            // the escapes walked away from it — which in Oak's laboratory means
            // walking back into the rows that soft-lock the cartridge
            // (`docs/gen1/sharp-edges.md`), over and over, at even odds. A
            // sidestep that also closes distance is strictly the better one;
            // the coin flip is kept for the case where that axis is already
            // level, which is the only case where neither direction is better.
            let dx = wx as i32 - s.x as i32;
            let dy = wy as i32 - s.y as i32;
            let r = self.roll(2);
            let d = if dx.abs() >= dy.abs() {
                toward("up", "down", dy, r)
            } else {
                toward("left", "right", dx, r)
            };
            let n = 2 + self.roll(3) as u32;
            return Plan::new("Walls in the way — go round.")
                .walk(d, n)
                .finish();
        }

        if !self.completed {
            let (wx, wy) = st.via[self.idx];
            let budget = (dist((s.x, s.y), (wx, wy)) + 6 + self.roll(5) as u32).min(24);
            // **Diversity by detour, never by skipping a waypoint.** An
            // earlier version dropped a turning point at random so the
            // executor would path the corner itself; on open ground that is
            // free variety, and in a corridor the turning point *is* the
            // corridor. Measured on Viridian City, whose row 27 is
            // `#.###.##############` — two gaps in twenty tiles — where
            // skipping (19,28) left the solver oscillating on row 28 for 600
            // decisions.
            //
            // **But a wander in ANY direction can change the route, and that
            // claim used to be made here without one.** `GoTo` is greedy on the
            // larger remaining delta and commits to a run along the other axis
            // when it is blocked, so where it starts from decides which
            // obstacle it meets. Measured in Oak's laboratory: one wandered
            // step from (5,4) up to (5,3) put a Poké Ball sprite between the
            // player and the table, the detour that followed ran south into the
            // rows that soft-lock the cartridge (`docs/gen1/sharp-edges.md`),
            // and the attempt was over — twice in 600, at a bar of one in a
            // thousand.
            //
            // So a wander steps **towards** the waypoint, never past it and
            // never away: it picks one of the axes that still has distance on
            // it and walks part of the way. That is a genuinely different route
            // where the ground is open — which axis is walked first, and how
            // far — and it cannot leave the rectangle the `goto` was going to
            // cross anyway, so it cannot introduce an obstacle the direct path
            // would not have met.
            let p = Plan::new(&format!("Head for ({wx},{wy})."));
            let p = if self.roll(6) == 0 {
                let dx = wx as i32 - s.x as i32;
                let dy = wy as i32 - s.y as i32;
                let mut axes: Vec<(&str, u32)> = Vec::new();
                if dx != 0 {
                    axes.push((if dx > 0 { "right" } else { "left" }, dx.unsigned_abs()));
                }
                if dy != 0 {
                    axes.push((if dy > 0 { "down" } else { "up" }, dy.unsigned_abs()));
                }
                match axes.len() {
                    0 => p,
                    n => {
                        let (d, room) = axes[self.roll(n as u64) as usize];
                        p.walk(d, 1 + self.roll(room.min(3) as u64) as u32)
                    }
                }
            } else {
                p
            };
            return p.goto(wx, wy, budget).finish();
        }

        match st.finish {
            Finish::Arrive => Plan::new("Wait for the map to change.")
                .wait(40)
                .text(8)
                .finish(),
            Finish::Step(d) => Plan::new(&format!("Step {d} out of here."))
                .walk(d, 2)
                .finish(),
            // A talk finishes its stage the moment it is performed: the next
            // thing on this map is whatever comes after the conversation.
            Finish::Grind(a, b) => {
                let (x, y) = if dist((s.x, s.y), a) > dist((s.x, s.y), b) {
                    a
                } else {
                    b
                };
                Plan::new("Pace the grass looking for a fight.")
                    .goto(x, y, 20)
                    .finish()
            }
            Finish::Talk(d) => {
                let e = self.stage.entry(s.map).or_insert(0);
                *e += 1;
                self.idx = 0;
                self.completed = false;
                Plan::new(&format!("Face {d} and talk."))
                    .press(d, 2)
                    .press("a", 1)
                    .text(24)
                    .finish()
            }
        }
    }

    /// Somewhere the route does not mention. The exits the observer can see
    /// are enough to get back: take the one that leads to a map on the route,
    /// and failing that the one that leads anywhere at all.
    fn off_route(&mut self, s: &Snap) -> String {
        let back = s
            .exits
            .iter()
            .find(|(_, _, to)| self.on_route.contains(to))
            .or_else(|| s.exits.first());
        match back {
            Some(&(x, y, _)) => Plan::new("Off the route — back out through the door.")
                .goto(x, y, 20)
                .walk("down", 2)
                .finish(),
            None => {
                let dirs = ["up", "down", "left", "right"];
                let d = dirs[self.roll(4) as usize];
                Plan::new("Off the route with no door in sight — explore.")
                    .walk(d, 4)
                    .finish()
            }
        }
    }
}

/// Which way to step on one axis: towards the waypoint while there is any
/// distance left on it, and only then the coin flip `r`.
fn toward(neg: &'static str, pos: &'static str, delta: i32, r: u64) -> &'static str {
    match (delta.signum(), r) {
        (-1, _) => neg,
        (1, _) => pos,
        (_, 0) => neg,
        _ => pos,
    }
}

// ---------------------------------------------------------------------------
// battle telemetry
// ---------------------------------------------------------------------------

/// What the battle policy actually did, aggregated across every attempt in a
/// run.
///
/// **A link can pass while the battle policy is quietly awful**, so the pass
/// rate is not enough: a win at 2 HP is a loss on the next link, a policy that
/// wins by burning every potion has solved nothing, and running is only a
/// decision if somebody checks whether it was the right one. The counters are
/// process-global and relaxed because they are read once, at the end, and
/// `corpus` fans attempts across threads.
mod tally {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    macro_rules! counters {
        ($($n:ident),* $(,)?) => {
            $(pub static $n: AtomicU64 = AtomicU64::new(0);)*
        };
    }
    counters!(
        BATTLES,
        WON,
        LOST,
        FLED,
        FAINTS,
        ITEMS,
        RUN_FORBIDDEN,
        RUN_FAINTING,
        RUN_TRAINED,
        RUN_HURT,
        RUN_FAILED,
        DOUBLE_FAIL,
        NEAR_DEATH,
        HP_LEFT_PERMILLE,
        HP_SAMPLES,
        DMG_CHECKED,
        DMG_IN_RANGE,
        DMG_UNDER,
        DMG_OVER,
        DMG_KO_OK,
        DMG_KO_SHORT,
        TURNS,
        STAT_TURNS,
        UNRESOLVED,
    );

    pub fn bump(c: &AtomicU64, by: u64) {
        c.fetch_add(by, Relaxed);
    }
    pub fn get(c: &AtomicU64) -> u64 {
        c.load(Relaxed)
    }

    /// One line, printed beside the pass rate.
    pub fn report() -> Option<String> {
        let n = get(&BATTLES);
        if n == 0 {
            return None;
        }
        let pct = |a: u64| 100.0 * a as f64 / n as f64;
        let hp = if get(&HP_SAMPLES) > 0 {
            get(&HP_LEFT_PERMILLE) as f64 / get(&HP_SAMPLES) as f64 / 10.0
        } else {
            0.0
        };
        let checked = get(&DMG_CHECKED).max(1);
        Some(format!(
            "battles {n}: won {} ({:.1}%), lost {} ({:.1}%), fled {} ({:.1}%), \
             {} still running when the attempt ended; \
             {:.1} turns each\n  \
             HP left when it ended: {hp:.1}% of max ({} sample(s)); faints {}; \
             items used {}\n  \
             ran because: {} the leg forbids a win, {} one hit from fainting, \
             {} the lead is already trained, {} too hurt to pick a fight; \
             {} escapes failed, {} battle(s) had two fail in a row, {} battle(s) \
             left the lead one hit from death\n  \
             turns spent lowering a stat: {}\n  \
             damage predictions: {} landed short of a knockout, {:.1}% inside \
             the roll range, {} under, {} over (a critical hit lands over); \
             {} knocked the target out, where the hit is only seen as the HP it \
             had left — {} of those were short of the model's own maximum",
            get(&WON),
            pct(get(&WON)),
            get(&LOST),
            pct(get(&LOST)),
            get(&FLED),
            pct(get(&FLED)),
            get(&UNRESOLVED),
            get(&TURNS) as f64 / n as f64,
            get(&HP_SAMPLES),
            get(&FAINTS),
            get(&ITEMS),
            get(&RUN_FORBIDDEN),
            get(&RUN_FAINTING),
            get(&RUN_TRAINED),
            get(&RUN_HURT),
            get(&RUN_FAILED),
            get(&DOUBLE_FAIL),
            get(&NEAR_DEATH),
            get(&STAT_TURNS),
            get(&DMG_CHECKED),
            100.0 * get(&DMG_IN_RANGE) as f64 / checked as f64,
            get(&DMG_UNDER),
            get(&DMG_OVER),
            get(&DMG_KO_OK) + get(&DMG_KO_SHORT),
            get(&DMG_KO_SHORT),
        ))
    }
}

/// Per-driver battle bookkeeping, folded into the global counters as it goes.
#[derive(Default)]
struct BattleTally {
    /// The last view seen, so the end of a battle can be recognised by its
    /// absence and scored from what it looked like just before.
    last: Option<terminalgb_plugin_agent::battle::View>,
    /// The damage range we predicted for the move just chosen, and their HP
    /// when we chose it — the pair a prediction is checked against.
    predicted: Option<(u16, u16, u16)>,
    turns: u64,
    /// Turns of this battle already spent lowering a stat.
    stat_turns: u8,
    ran: bool,
    /// A RUN was chosen on the turn just gone; if the battle is still here on
    /// the next one, the escape failed and cost a turn.
    ran_last_turn: bool,
    /// Escapes that failed in *this* battle, so a run of two can be counted.
    fails_here: u8,
    /// Has this battle already been counted as a near-death?
    near_here: bool,
    /// Has this driver made a decision yet?
    primed: bool,
    /// This battle was already in progress when the run started, so it is the
    /// *previous* link's and counting it here would report it twice.
    inherited: bool,
}

impl BattleTally {
    /// Called once per battle decision with the state it was made from.
    fn observe(&mut self, v: &terminalgb_plugin_agent::battle::View) {
        use std::sync::atomic::Ordering::Relaxed;
        if self.last.is_none() {
            // **Disown a battle that was already running at the first
            // decision** — the same call `scenario::run::Battles` makes for the
            // referee, and for the same reason. Link 11's goal is
            // `lead_level>=10`, which fires the instant the level ticks over,
            // and that happens inside the battle that earned it: so every link
            // 12 run opens on "CHARMANDER grew to level 10!" with the opponent
            // already at 0 HP. Counted here, that battle is reported twice —
            // once by the link that fought it and once by the link that
            // inherited it, which read as **60 wins on a leg that runs from
            // everything**.
            self.inherited = !self.primed;
            if !self.inherited {
                tally::bump(&tally::BATTLES, 1);
            }
            self.turns = 0;
            self.stat_turns = 0;
            self.ran = false;
            self.ran_last_turn = false;
            self.fails_here = 0;
            self.near_here = false;
        }
        // **Check the prediction against what happened.** A prediction nobody
        // verified is a guess with a decimal point.
        //
        // **Held until their HP actually moves**, because `observe` runs on
        // every decision and a move takes several of them to resolve — taking
        // the prediction on the first one threw every single check away, and
        // the report said "0 checked" while looking perfectly healthy.
        if let Some((lo, hi, before)) = self.predicted {
            match v.theirs.hp.cmp(&before) {
                std::cmp::Ordering::Less => {
                    self.predicted = None;
                    let dealt = before - v.theirs.hp;
                    // **A knockout censors the observation.** HP stops at zero,
                    // so a hit that was going to do 12 to a Pokémon with 5 left
                    // is only ever seen as 5 — and comparing that against the
                    // model's minimum scores every overkill as the model being
                    // wrong. It read 73.9% in range on a leg that wins three
                    // fights in four, against 81.3% on a leg of trainer battles
                    // where the same overkills are a smaller share. The true
                    // statement about a knockout is "at least `before`", so it
                    // agrees with the model exactly when the model's own
                    // maximum reached that far, and it is counted apart rather
                    // than folded in to flatter the percentage.
                    if v.theirs.fainted() {
                        tally::bump(
                            if hi >= before {
                                &tally::DMG_KO_OK
                            } else {
                                &tally::DMG_KO_SHORT
                            },
                            1,
                        );
                    } else {
                        tally::bump(&tally::DMG_CHECKED, 1);
                        tally::bump(
                            match dealt {
                                d if d < lo => &tally::DMG_UNDER,
                                d if d > hi => &tally::DMG_OVER,
                                _ => &tally::DMG_IN_RANGE,
                            },
                            1,
                        );
                    }
                }
                // They were healed or replaced; the prediction is stale.
                std::cmp::Ordering::Greater => self.predicted = None,
                std::cmp::Ordering::Equal => {}
            }
        }
        // **The precursors, not the outcome.** A death needs the lead one hit
        // from dying AND an escape failing twice in the same battle, and both
        // happen roughly a hundred times more often than a death does. At about
        // one failure per 3,000 attempts no variant can be told from another by
        // its pass rate; these can be compared on hundreds of events.
        if !self.near_here && v.ours.hp > 0 && v.ours.hp <= v.incoming_max {
            self.near_here = true;
            tally::bump(&tally::NEAR_DEATH, 1);
        }
        self.last = Some(v.clone());
        let _ = Relaxed;
    }

    /// Called with the reason string of whatever was chosen.
    ///
    /// **Runs are counted by the reason the policy gave, not graded.** The
    /// policy only ever runs for three reasons and all three are legitimate by
    /// construction, so a right/wrong column would be vacuous; what a reader
    /// needs is the mix, plus how often the escape failed, which costs a turn
    /// and is the real price of the decision.
    fn note(&mut self, why: &str, chose: Option<usize>) {
        // A turn is an action chosen, not a decision spent — most decisions in
        // a battle go on reading the text a turn produced.
        self.turns += 1;
        // **An escape failed if we are choosing again in the same battle.**
        // Checking it on the next *observation* instead counted every run as a
        // failure, because a run attempt is always followed by the text that
        // reports it: 5,288 fled battles reported 5,372 failed escapes. The
        // real figure is the 84 that had to try twice.
        if self.ran_last_turn {
            tally::bump(&tally::RUN_FAILED, 1);
            self.fails_here += 1;
            if self.fails_here == 2 {
                tally::bump(&tally::DOUBLE_FAIL, 1);
            }
        }
        // **Match the policy's named reason, not a fragment of its prose.**
        // These were substring guesses, which is tolerable for a report and was
        // not once one of them started steering the walk to the nurse.
        use terminalgb_plugin_agent::battle::why as w;
        let counter = match why {
            w::FORBIDDEN => Some(&tally::RUN_FORBIDDEN),
            w::FAINTING => Some(&tally::RUN_FAINTING),
            w::TRAINED => Some(&tally::RUN_TRAINED),
            w::TOO_HURT => Some(&tally::RUN_HURT),
            _ => None,
        };
        if let Some(c) = counter {
            self.ran = true;
            tally::bump(c, 1);
        }
        self.ran_last_turn = counter.is_some();
        if why.contains("bag can outrun") {
            tally::bump(&tally::ITEMS, 1);
        }
        if why.contains("lower what it hits with") {
            self.stat_turns = self.stat_turns.saturating_add(1);
            tally::bump(&tally::STAT_TURNS, 1);
        }
        // **The move the policy actually chose, not the first damaging one in
        // the list.** This used to be
        // `v.moves.iter().find(|m| m.damaging() && m.usable())`, which is the
        // same move whenever the best one is in slot 1 — true for every link
        // before the gym, because a starter's only attack is slot 1. In
        // Pewter it is not: a SQUIRTLE holds TACKLE, TAIL WHIP, BUBBLE and the
        // policy picks BUBBLE, which is four times effective on ROCK/GROUND —
        // so the range recorded was TACKLE's and the damage observed was
        // BUBBLE's. Measured on the first 120-attempt gym run: **474 of 700
        // predictions landed "over"**, against the 5-8 per cent a critical-hit
        // rate explains, and the column read 29.9 per cent in range on a
        // policy that was in fact fine.
        if let (Some(v), Some(slot)) = (&self.last, chose) {
            if let Some(m) = v.moves.iter().find(|m| m.slot == slot) {
                if why.contains("knocks it out")
                    || why.contains("most damage")
                    || why.contains("ends it first")
                {
                    self.predicted = Some((m.min, m.max, v.theirs.hp));
                }
            }
        }
    }

    /// Called every decision; scores a battle the moment it is over.
    fn tick(&mut self, in_battle: bool) {
        // Every decision runs through here, which is what makes it the right
        // place to record that the run has started — `observe` only sees the
        // decisions that have a battle in them, so priming there would disown
        // the first real battle of any run that begins in the overworld.
        self.primed = true;
        if in_battle {
            return;
        }
        let Some(v) = self.last.take() else { return };
        if self.inherited {
            self.inherited = false;
            self.predicted = None;
            self.turns = 0;
            self.stat_turns = 0;
            self.ran_last_turn = false;
            return;
        }
        tally::bump(&tally::TURNS, self.turns);
        if self.ran {
            tally::bump(&tally::FLED, 1);
        } else if v.ours.hp == 0 {
            tally::bump(&tally::LOST, 1);
            tally::bump(&tally::FAINTS, 1);
        } else {
            tally::bump(&tally::WON, 1);
        }
        // HP left at the end, which is what the next link inherits.
        if v.ours.max_hp > 0 {
            tally::bump(
                &tally::HP_LEFT_PERMILLE,
                (v.ours.hp as u64 * 1000 / v.ours.max_hp as u64).min(1000),
            );
            tally::bump(&tally::HP_SAMPLES, 1);
        }
        self.predicted = None;
        self.turns = 0;
        self.stat_turns = 0;
        self.ran_last_turn = false;
    }
}

/// **Score the last battle even when the run ends inside it — but only if the
/// driver saw who won.**
///
/// A link whose goal fires on the turn a battle ends would otherwise contribute
/// nothing to the battle report, and the report would quietly describe a
/// smaller sample than the run. That is what this is for, and it worked
/// because in an ordinary win or loss the driver keeps being asked for
/// decisions while the "…fainted!" text plays, so the last view it holds is
/// already decisive.
///
/// **A run cut short mid-battle has no such view, and scoring it anyway
/// reports a loss as a win.** [`BattleTally::tick`] reads "we are still
/// standing" as a win, and when the harness stops on `party_hp=0` the driver's
/// last decision was taken one hit before the end — so the lead is standing in
/// that view and dead in the machine. Measured on the first gym attempt this
/// was written for: a level 10 BULBASAUR fainted to the gym's junior trainer,
/// its final state reading `wIsInBattle = 2` and 0/30 HP, and the report said
/// `battles 1: won 1 (100.0%) … faints 0`. Every number in it was wrong, on
/// exactly the link whose battle numbers are the point.
///
/// So an indecisive view is counted as **unresolved** and named as such.
/// Nothing else moves: link 05's rival battle ends in the overworld with the
/// flag, so its decisions run past the knockout and its view is decisive.
impl Drop for ChainDriver {
    fn drop(&mut self) {
        let decisive = self
            .tally
            .last
            .as_ref()
            .map(|v| v.ours.hp == 0 || v.theirs.hp == 0)
            .unwrap_or(false);
        if decisive {
            self.tally.tick(false);
        } else if self.tally.last.is_some() && !self.tally.inherited {
            tally::bump(&tally::UNRESOLVED, 1);
        }
    }
}

fn dist(a: (u8, u8), b: (u8, u8)) -> u32 {
    (a.0 as i32 - b.0 as i32).unsigned_abs() + (a.1 as i32 - b.1 as i32).unsigned_abs()
}

impl Driver for ChainDriver {
    fn name(&self) -> &str {
        "expert-route"
    }

    fn submit(&mut self, request: &str) -> bool {
        if self.busy {
            return false;
        }
        let s = Snap::parse(request);
        let reply = self.decide(&s);
        // A stuck leg is diagnosed from where it was standing and what it
        // thought it was doing, and neither is in the recording. `GB_CHAIN_
        // TRACE=1` is how every finding in `docs/agent/pewter-chain.md` was
        // arrived at.
        if std::env::var_os("GB_CHAIN_TRACE").is_some() {
            let text = s
                .text
                .replace('\n', " | ")
                .chars()
                .take(220)
                .collect::<String>();
            let short = reply.chars().take(110).collect::<String>();
            let arena = match &s.arena {
                Some(a) => format!(
                    " {{us {}/{} spd{} | them {}/{} spd{} | cursor {} | in {}/{} | \
                     moves {:?}}}",
                    a.ours.hp,
                    a.ours.max_hp,
                    a.ours.speed,
                    a.theirs.hp,
                    a.theirs.max_hp,
                    a.theirs.speed,
                    a.menu_item,
                    a.incoming_expected,
                    a.incoming_max,
                    a.moves
                        .iter()
                        .map(|m| (m.slot, m.id, m.pp, m.min, m.max))
                        .collect::<Vec<_>>()
                ),
                None => String::new(),
            };
            eprintln!(
                "  [{:3}] map 0x{:02X} ({:2},{:2}) stage {} idx {} kind {:8} lv{} \
                 hp{}/{} ::{arena} [{text}] {short}",
                self.calls,
                s.map,
                s.x,
                s.y,
                self.stage.get(&s.map).copied().unwrap_or(0),
                self.idx,
                s.kind,
                s.lead_level,
                s.lead_hp,
                s.lead_max_hp,
            );
        }
        self.pending = Some(reply);
        self.busy = true;
        self.calls += 1;
        true
    }

    fn poll(&mut self) -> Poll {
        if !self.busy {
            return Poll::Idle;
        }
        self.busy = false;
        match self.pending.take() {
            Some(r) => Poll::Ready(r),
            None => Poll::Idle,
        }
    }

    fn cancel(&mut self) {
        self.busy = false;
        self.pending = None;
    }
}

// ---------------------------------------------------------------------------
// front end
// ---------------------------------------------------------------------------

const USAGE: &str = "\
pewter_chain — twelve scenarios from a cold cartridge to Pewter City

  solve --rom R --scenario S [--start FILE] [--seed N] [--out DIR] [--quiet]
      Play one link with the expert solver and write its recording.

  chain --rom R [--seeds N] [--out DIR] [--from N] [--to N]
      Play the whole chain, carrying each link's finish forward as the next
      link's start. Writes states/NN.gbss and one recording per attempt.

  corpus --rom R --scenario S --start FILE --runs N [options]
      Many divergent attempts at one link, fanned out across threads.
      Successes and failures are both kept.
      --seed0 N     first seed (default 0)
      --workers N   default: cores - 4, because other things share this box
      --primitive   one button per decision, which is the shape a learner needs
      --start-dir D a pool of entry states; a seed takes `seed % pool`
      --idle-spread N  frames of phase to spread attempts over (default 3001,
                    0 disables). Attempts that differ in nothing the cartridge
                    can see are one attempt counted N times; the run fails
                    loudly if any two trajectories match.
      --exits N     keep the first N successful exits as <out>/exits/*.gbss,
                    which is the next link's entry pool
      --paths       also write where the player was, per frame, per run

  film --rom R --agents N [--out DIR] [--workers N] [--from N] [--to N]
      Play the WHOLE chain once per agent, each agent carrying its own exit
      state forward, and write one continuous per-frame position track each.
      An agent that fails a link stops there, which is the honest picture.
      --from/--to select a slice of the chain. The gym arc films as its own
      cut rather than being sewn onto the journey to Pewter — see
      docs/agent/brock.md — so a command that always plays every link
      cannot produce either film on purpose.

  sprites --rom R --start FILE --out DIR
      Read the player's own overworld sprite out of OAM and VRAM, four
      facings, as transparent PNGs. Game art: write it outside the repository.

  ab --rom R [--plan FILE] [--scenario S] [--start-dir D] [--runs N]
     [--lead NAME|N ...] [--split seen|unseen|n/a] [--student \"CMD ARGS\"]
      THE TEST RIG. The same fight, from the same entry states, at the same
      seeds and the same phase offsets, played by two or more players, with
      the rates, the intervals and the paired gap between them.
      --plan FILE   a `gbtestrig/1` plan; every field below overrides it
      --lead X      one subject per lead species (a name or an index); repeat
      --split S     seen | unseen | n/a — REQUIRED for a trained player, and
                    printed at the top, because a \"trained on this fight\"
                    number quoted as generalisation is the worst outcome here
      --student CMD add a subprocess arm (see plugins/agent/src/subprocess.rs)

  abfilm --rom R --plan FILE [--seed N] [--lead X] [--runs FRAMES] [--out DIR]
      The same fight, the same entry state, the same seed, every arm side by
      side, as a PNG sequence. A table says a player is behind; only a picture
      says whether it is battling badly or standing in a doorway.
      --runs FRAMES  how many output frames to aim for (default 600, i.e. 20 s
                     at 30 fps). The stride is derived from the LONGEST arm and
                     rounded up, and the timelapse ratio is printed — the arms
                     differ in length by an order of magnitude, so a fixed
                     stride gives a five-minute film of one of them flailing.

  verify --rom R --scenario S [--start FILE] [--runs N] [--primitive]
      Play the same seeds serially and in parallel and require identical
      final states. Determinism outranks everything else in flight.

  states --rom R --start-dir D
      What a pool of entry states actually holds — lead species, level, HP,
      moves, money and badges — and the distribution across the pool.
      **A link's precondition is a claim about its entry pool**, and this is
      how that claim gets checked rather than assumed. No frames are
      emulated: a save state carries the party.
";

#[derive(Default)]
struct Args {
    rom: Option<PathBuf>,
    scenario: Option<PathBuf>,
    start: Option<PathBuf>,
    out: Option<PathBuf>,
    seed: u64,
    seed0: u64,
    runs: u32,
    seeds: u32,
    from: usize,
    to: usize,
    quiet: bool,
    workers: u32,
    /// Whether `--workers` was actually typed. Comparing the value against
    /// `default_workers()` cannot tell "not given" from "given the default",
    /// so a plan's own `workers` used to survive an explicit `--workers 16` on
    /// a twenty-core box. A flag says what was meant.
    workers_given: bool,
    primitive: bool,
    paths: bool,
    start_dir: Option<PathBuf>,
    /// Frames of phase to spread attempts over; 0 disables the axis.
    idle_spread: u32,
    /// How many of a corpus's successful exits to keep as `.gbss`, for the
    /// next link to use as its entry pool.
    exits: u32,
    /// A `gbtestrig/1` plan file for `ab`.
    plan: Option<PathBuf>,
    /// One `ab` subject per entry, each a lead species.
    leads: Vec<String>,
    /// `seen`, `unseen` or `n/a`; see `abrig::Split`.
    split: Option<String>,
    /// A subprocess arm for `ab`: the program and its arguments, one string.
    student: Option<String>,
    /// Extra raw addresses for `states`, as `NAME@0xADDR` or `0xADDR`.
    /// **The verification tool for a new goal**: a flag or a byte a link is
    /// about to be judged on gets read out of real states first, so the
    /// goal is a measurement rather than a guess.
    peek: Vec<String>,
}

fn parse(args: &[String]) -> Args {
    let mut a = Args {
        seed: 1,
        seeds: 1,
        runs: 1,
        from: 1,
        to: 99,
        workers: default_workers(),
        idle_spread: IDLE_SPREAD,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: usize| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("{} needs a value", args[i]);
                exit(2);
            })
        };
        match args[i].as_str() {
            "--rom" => a.rom = Some(PathBuf::from(next(i))),
            "--scenario" => a.scenario = Some(PathBuf::from(next(i))),
            "--start" => a.start = Some(PathBuf::from(next(i))),
            "--start-dir" => a.start_dir = Some(PathBuf::from(next(i))),
            "--idle-spread" => a.idle_spread = next(i).parse().unwrap_or(IDLE_SPREAD),
            "--exits" => a.exits = next(i).parse().unwrap_or(0),
            "--plan" => a.plan = Some(PathBuf::from(next(i))),
            "--lead" => a.leads.push(next(i)),
            "--split" => a.split = Some(next(i)),
            "--peek" => a.peek.push(next(i)),
            "--student" => a.student = Some(next(i)),
            "--out" => a.out = Some(PathBuf::from(next(i))),
            "--seed" => a.seed = next(i).parse().unwrap_or(1),
            "--seed0" => a.seed0 = next(i).parse().unwrap_or(0),
            "--runs" => a.runs = next(i).parse().unwrap_or(1).max(1),
            "--seeds" => a.seeds = next(i).parse().unwrap_or(1).max(1),
            "--from" => a.from = next(i).parse().unwrap_or(1),
            "--to" => a.to = next(i).parse().unwrap_or(12),
            "--workers" => {
                a.workers = next(i).parse().unwrap_or(1).max(1);
                a.workers_given = true;
            }
            "--quiet" => {
                a.quiet = true;
                i += 1;
                continue;
            }
            "--primitive" => {
                a.primitive = true;
                i += 1;
                continue;
            }
            "--paths" => {
                a.paths = true;
                i += 1;
                continue;
            }
            other => {
                eprintln!("unknown option {other:?}\n\n{USAGE}");
                exit(2);
            }
        }
        i += 2;
    }
    a
}

/// **Never run against a ROM where it lives** — the MBC writes a `.sav` beside
/// it, and the captain's cartridges are on this machine.
/// The cartridge image, read **once** for a whole command.
///
/// It used to be read from the player's own `~/roms` and written back out to
/// `<out>/cart.gb` on **every attempt**, so a 3,000-attempt certification moved
/// six gigabytes to produce three thousand identical copies of one megabyte.
/// The copy existed for one reason — `Gameboy::new(.., Some(path))` writes
/// `<rom>.sav` beside the cartridge, and a benchmark must never write beside
/// somebody's game — and no attempt has ever read a `.sav`: an attempt's state
/// arrives as a save state and leaves as one. So there is nothing to stage.
#[derive(Clone)]
struct Cart(std::sync::Arc<Vec<u8>>);

fn load_cart(rom: &Path) -> Cart {
    Cart(std::sync::Arc::new(std::fs::read(rom).unwrap_or_else(
        |e| {
            eprintln!("cannot read {}: {e}", rom.display());
            exit(1);
        },
    )))
}

struct Outcome {
    goal: bool,
    frames: u64,
    decisions: usize,
    ending: String,
    detail: String,
    map: u8,
    x: u8,
    y: u8,
    wall_ms: u128,
    /// Of `wall_ms`, how much was inside the emulator. See
    /// `RunResult::emulate_ms`.
    emulate_ms: u128,
    /// Building the machine, burning the phase offset and loading the start
    /// state — the per-attempt tax an *episode* pays, and the number that says
    /// whether short episodes are affordable. Measured around exactly that work
    /// and nothing else.
    setup_ms: u128,
    frames_total: u64,
    state: Vec<u8>,
    /// How the battles went, as the **host** saw them — so an arm that is not
    /// the scripted expert still has a battle column. See
    /// `terminalgb::scenario::run::BattleSummary`.
    battles: terminalgb::scenario::run::BattleSummary,
    recording: Recording,
    path: Vec<terminalgb::scenario::run::PathPoint>,
}

/// Everything an attempt *did*, in one number.
///
/// The decision replies in order, the frame each landed on, how long the run
/// was and where it ended. Two attempts sharing this played the same game: not
/// merely the same verdict, the same buttons at the same moments.
fn trajectory_hash(o: &Outcome) -> u64 {
    // FNV-1a, hand-rolled like every other hash in this repository — the
    // dependency count is a contract `make embed` measures.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01b3);
        }
    };
    eat(&o.frames.to_le_bytes());
    eat(&(o.decisions as u64).to_le_bytes());
    eat(&[o.map, o.x, o.y]);
    eat(o.ending.as_bytes());
    eat(&o.recording.final_state.to_le_bytes());
    for d in &o.recording.decisions {
        eat(&d.frame.to_le_bytes());
        eat(d.reply.as_bytes());
    }
    h
}

/// Frames of phase a certification run spreads its attempts over.
///
/// **3001, and it is a measurement rather than a round number.** AgentGB
/// reached the same constant from the other end and wrote down why: every
/// state in a start pool is frozen during one traversal, so the town's
/// wandering people are in nearly the same place in all of them, and a hundred
/// frames is not long enough for anyone to move. Same policy, same tile, same
/// 150 attempts scored **100% under a 0-96 frame offset and 52.7% under a
/// 0-3000 one**, with 6 distinct people-layouts seen against 22. A prime, so
/// `seed * PHASE_STRIDE % IDLE_SPREAD` is a bijection and 3000 seeds give 3000
/// distinct phases.
const IDLE_SPREAD: u32 = 3001;
/// How often the rig says how far it has got.
///
/// **Five seconds, and it is on by default.** A long run that prints nothing
/// until it finishes cannot be told from a wedged one, and this rig exists to
/// be driven by things that are not people. `--quiet` silences the human line;
/// `<out>/progress.ndjson` is written either way.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_secs(5);
/// Coprime with [`IDLE_SPREAD`], so consecutive seeds land far apart.
const PHASE_STRIDE: u32 = 337;

/// The phase offset attempt `seed` runs at.
///
/// One expression, used by every subcommand, so `solve --seed N` reproduces
/// `corpus`'s seed N exactly and `verify` exercises the phased path rather
/// than a special-cased unphased one.
fn phase_for(seed: u64, spread: u32) -> u32 {
    if spread == 0 {
        return 0;
    }
    (seed as u32).wrapping_mul(PHASE_STRIDE) % spread
}

/// Advance the machine `idle` frames and take the state it reaches.
///
/// **This is what makes two attempts independent, and without it a corpus is
/// one attempt copied N times.** Emulation here is a pure function of (start
/// state, scenario, seed) — that is the property `verify` exists to defend —
/// so a certification loop that holds the first two fixed and varies only a
/// seed the driver may never consult produces N byte-identical runs. Measured:
/// the first 3000-attempt run of `01-new-game` reported 3000 of 3000 and was
/// **one** attempt, 18 decisions and a single final-state hash, 3000 times.
/// The rule of three bounds nothing when the sample size is really one.
///
/// A phase offset is the axis that always exists. A start tile needs a pool
/// and a seed needs the driver to reach a seeded branch, but every scenario
/// can be entered a few frames later than last time — which is what a person
/// does, and which moves `rDIV`, the frame counter, and therefore every draw
/// Gen 1's `Random_` makes and every NPC in the town.
fn phase_start(gb: &mut Gameboy, start: Option<Vec<u8>>, idle: u32) -> Option<Vec<u8>> {
    if idle == 0 {
        return start;
    }
    if let Some(bytes) = &start {
        if let Err(e) = gb.load_state(bytes) {
            eprintln!("the start state was rejected while phasing: {e:?}");
            exit(1);
        }
    }
    for _ in 0..idle {
        gb.frame();
    }
    Some(gb.save_state())
}

/// Play one attempt.
///
/// Nine arguments, and they stay nine: the alternative is a struct whose
/// fields are set positionally at the call site anyway, and every one of these
/// is named at every call.
///
/// `idle` is the **phase offset**: frames burned from the start state before
/// the agent is allowed its first decision. See [`phase_start`].
#[allow(clippy::too_many_arguments)]
fn play(
    rom: &Cart,
    scenario_path: &Path,
    start: Option<Vec<u8>>,
    seed: u64,
    out: &Path,
    quiet: bool,
    primitive: bool,
    track: bool,
    idle: u32,
) -> Outcome {
    play_with(
        rom,
        scenario_path,
        start,
        out,
        quiet,
        track,
        idle,
        &|name| Box::new(ChainDriver::new(name, seed).primitive(primitive)),
        None,
    )
}

/// Play one attempt with **whatever** is pressing the buttons.
///
/// [`play`] is this with the scripted expert wired in. The split exists for the
/// A/B rig: an arm is a driver and nothing else, so the entry state, the phase
/// offset, the scenario, the referee, the sampling clock and the battle
/// bookkeeping are all literally the same code for every player. **A comparison
/// whose two halves ran through different harnesses is not a comparison**, and
/// the cheapest way to guarantee they did not is to have one harness.
#[allow(clippy::too_many_arguments)]
fn play_with(
    rom: &Cart,
    scenario_path: &Path,
    start: Option<Vec<u8>>,
    out: &Path,
    quiet: bool,
    track: bool,
    idle: u32,
    driver: &dyn Fn(&str) -> Box<dyn Driver>,
    frames_dir: Option<&Path>,
) -> Outcome {
    let s = Scenario::load(scenario_path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", scenario_path.display());
        exit(1);
    });
    let setup_at = std::time::Instant::now();
    std::fs::create_dir_all(out).expect("create output directory");
    // **No save path, so no battery.** Cartridge RAM comes in and goes out
    // inside the save state; a `.sav` beside a staged copy is a file nobody
    // reads, and `Battery::flush` writes it through `write_atomic`, which
    // fsyncs the file *and* its directory. Measured on this box, one worker,
    // twenty attempts of link 01: the whole run is 23.0 s with the output
    // directory on disk and 12.4 s with it on tmpfs, for identical CPU time
    // (12.25 s against 12.33 s) — the filesystem was 46 % of the wall clock.
    // The mapper still comes from header `$0147` either way (`mbc::get_mbc`),
    // and the trajectory hashes are unchanged, checked in both directions.
    let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
    // **A machine always has an APU, and a cartridge can tell.** The Pokémon
    // Center's healing sequence never finishes without one: `Gameboy::new`
    // leaves `MMU.sound` at `None`, which is a machine with **no sound chip**
    // rather than a quiet one, and the game's own audio engine then waits for
    // a jingle that can never end. Measured: 700 decisions parked at (3,3)
    // with "OK. We'll need your POKéMON." on screen and every walk refused.
    // The same rule `testharness/testroms.rs` records — never build a test
    // machine without an APU — and it costs nothing here, because
    // `SilentPlayer` throws the samples away.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    // **Declared here, not in `ScenarioRun`, because the phase is emulated
    // before the run exists.** A phase offset averages half the spread — 1,500
    // frames against an attempt's 3,500 — so a third of everything this program
    // emulates happens in `phase_start`, and leaving the declaration to the
    // runner left that third drawing pixels and resampling audio for nobody.
    // Both skips are guest-invisible; `ScenarioRun::collecting` below repeats
    // them for the run itself and is what a caller who did not phase gets.
    // **A player that looks at the screen must be given one.** The pixel skip
    // below is worth about half a frame's cost and is right for every player
    // this repository has: the expert reads WRAM and a baseline reads nothing.
    // A trained policy reads pixels, and a machine that is not drawing hands it
    // whatever was last in the buffer — which is not a failure a rate would
    // ever explain. So the skip is conditional on nobody wanting the picture,
    // and the arm that wants it pays for it.
    gb.set_render_pixels(frames_dir.is_some());
    gb.set_render_audio(false);
    // **Phase the world before the agent is attached, never after.** Burning
    // the frames here means the plugin does not exist yet and so cannot act
    // during them; the machine is simply left `idle` frames further on, and
    // that state becomes the attempt's start. See [`phase_start`].
    let start = phase_start(&mut gb, start, idle);
    let setup_ms = setup_at.elapsed().as_millis();
    let cfg = Config {
        budget: Budget {
            max_calls: s.episode().max_calls,
            ..Budget::default()
        },
        objective: s.objective.clone(),
        // The snapshot carries `frame_png` when this is set, which is the
        // whole of what a screen-only player needs from this side.
        frame_capture: frames_dir.map(|d| d.to_path_buf()),
        ..Config::default()
    };
    gb.add_plugin(Box::new(AgentPlugin::new(driver(&s.name), cfg)))
        .expect("attach the agent");

    let mut run =
        ScenarioRun::new(gb, s.clone(), scenario_path, out.to_path_buf(), start)
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(1);
            })
            .tracking(track)
            // Nothing in this program looks at the picture or listens to the
            // sound: the solver reads `wTileMap` and a handful of WRAM bytes,
            // and a film is stitched from the position tracks. Both skips are
            // guest-invisible — see `ScenarioRun::collecting`.
            .collecting();
    if frames_dir.is_some() {
        // `collecting()` turns the picture off; an arm that reads it turns it
        // back on. Guest-invisible either way, and byte-identical at a frame
        // boundary — `Gameboy::set_render_pixels`.
        run.gameboy_mut().set_render_pixels(true);
    }
    if quiet {
        run = run.quiet().without_captures();
    }
    let r = run.play();
    Outcome {
        goal: r.reached_goal(),
        frames: r.recording.frames,
        decisions: r.recording.decisions.len(),
        ending: r.recording.ending.tag().to_string(),
        detail: r.lines().first().cloned().unwrap_or_default(),
        map: r.final_progress.map,
        x: r.final_progress.x,
        y: r.final_progress.y,
        wall_ms: r.wall_ms + setup_ms,
        emulate_ms: r.emulate_ms,
        setup_ms,
        // The phase offset is emulated too — with no plugin attached, so those
        // frames are pure emulation — and a throughput figure that leaves them
        // out is measuring less work than the run did.
        frames_total: r.recording.frames + idle as u64,
        state: r.final_state_bytes.clone(),
        battles: r.battles,
        recording: r.recording,
        path: r.path,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{USAGE}");
        exit(2);
    }
    let a = parse(&args[1..]);
    match args[0].as_str() {
        "ab" => cmd_ab(&a),
        "abfilm" => cmd_abfilm(&a),
        "solve" => cmd_solve(&a),
        "chain" => cmd_chain(&a),
        "corpus" => cmd_corpus(&a),
        "verify" => cmd_verify(&a),
        "sprites" => cmd_sprites(&a),
        "routes" => cmd_routes(),
        "states" => cmd_states(&a),
        "film" => cmd_film(&a),
        "-h" | "--help" | "help" => println!("{USAGE}"),
        other => {
            eprintln!("unknown subcommand {other:?}\n\n{USAGE}");
            exit(2);
        }
    }
}

/// **Leave a couple of cores.** Other crews share this box, and past the core
/// count extra workers only queue.
fn default_workers() -> u32 {
    std::thread::available_parallelism()
        .map(|n| (n.get() as u32).saturating_sub(4).max(1))
        .unwrap_or(4)
}

/// The entry states an attempt may begin from: `--start-dir`'s `.gbss` files
/// if given, else the single `--start`, else nothing (a cold cartridge).
fn entry_pool(a: &Args) -> Vec<Option<Vec<u8>>> {
    if let Some(d) = &a.start_dir {
        let mut v: Vec<PathBuf> = std::fs::read_dir(d)
            .unwrap_or_else(|e| {
                eprintln!("cannot read {}: {e}", d.display());
                exit(1);
            })
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "gbss").unwrap_or(false))
            .collect();
        v.sort();
        if v.is_empty() {
            eprintln!("no .gbss files in {}", d.display());
            exit(1);
        }
        return v
            .iter()
            .map(|p| Some(std::fs::read(p).expect("entry state")))
            .collect();
    }
    vec![a.start.as_ref().map(|p| {
        std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", p.display());
            exit(1);
        })
    })]
}

/// Where the player was, one row per **change**.
///
/// A row is written whenever any of map, position, facing, the walk counter or
/// the battle flag moves, so a standing character costs one row and a walking
/// one costs a row per frame — which is exactly the resolution a film needs and
/// no more. `frame` is the emulated frame the row begins at.
fn write_path(path: &Path, pts: &[terminalgb::scenario::run::PathPoint]) {
    let mut out = String::from("frame,map,x,y,facing,walking,battle\n");
    let mut last: Option<terminalgb::scenario::run::PathPoint> = None;
    for p in pts {
        let same = last.map(|l| {
            (l.map, l.x, l.y, l.facing, l.walking, l.in_battle)
                == (p.map, p.x, p.y, p.facing, p.walking, p.in_battle)
        });
        if same == Some(true) {
            continue;
        }
        out.push_str(&format!(
            "{},{},{},{},{},{},{}\n",
            p.frame, p.map, p.x, p.y, p.facing, p.walking, p.in_battle
        ));
        last = Some(*p);
    }
    let _ = std::fs::write(path, out);
}

fn need_rom(a: &Args) -> PathBuf {
    a.rom.clone().unwrap_or_else(|| {
        eprintln!("--rom is required");
        exit(2);
    })
}

fn cmd_solve(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let sp = a.scenario.clone().unwrap_or_else(|| {
        eprintln!("--scenario is required");
        exit(2);
    });
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/solve"));
    let start = a.start.as_ref().map(|p| {
        std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", p.display());
            exit(1);
        })
    });
    let o = play(
        &rom,
        &sp,
        start,
        a.seed,
        &out,
        a.quiet,
        a.primitive,
        a.paths,
        phase_for(a.seed, a.idle_spread),
    );
    report(&o, a.seed);
    if let Some(b) = tally::report() {
        println!("{b}");
    }
    if a.paths {
        write_path(&out.join(format!("path-{:05}.csv", a.seed)), &o.path);
    }
    let _ = o.recording.save(&out.join(format!("run-{}.json", a.seed)));
    let _ = std::fs::write(out.join("final.gbss"), &o.state);
}

fn report(o: &Outcome, seed: u64) {
    println!(
        "seed {seed}: {} — {} frames, {} decisions, {} ms, ended on map 0x{:02X} \
         ({}) at ({}, {})",
        if o.goal { "GOAL" } else { &o.detail },
        o.frames,
        o.decisions,
        o.wall_ms,
        o.map,
        terminalgb::longplay::map_name(o.map),
        o.x,
        o.y
    );
}

/// The whole chain, carrying each link's finish forward.
///
/// **Link 1 has no start state at all** — it boots the cartridge cold, from
/// the Nintendo logo. Every state after it is derived by playing forward, so
/// the whole chain is reproducible from nothing but the cartridge.
fn cmd_chain(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain"));
    let states = out.join("states");
    std::fs::create_dir_all(&states).expect("states directory");

    let files = chain_files();
    let mut carried: Option<Vec<u8>> = if a.from > 1 {
        let p = states.join(format!("{:02}.gbss", a.from - 1));
        Some(std::fs::read(&p).unwrap_or_else(|e| {
            eprintln!("link {} needs {}: {e}", a.from, p.display());
            exit(1);
        }))
    } else {
        None
    };

    for (n, f) in files.iter().enumerate() {
        let link = n + 1;
        if link < a.from || link > a.to {
            continue;
        }
        let sp = PathBuf::from(f);
        let name = sp.file_stem().unwrap().to_string_lossy().to_string();
        println!("\n=== link {link}: {name}");
        let dir = out.join(&name);
        let mut solved = None;
        for k in 0..a.seeds {
            let seed = 1 + k as u64;
            let o = play(
                &rom,
                &sp,
                carried.clone(),
                seed,
                &dir,
                true,
                a.primitive,
                a.paths,
                phase_for(seed, a.idle_spread),
            );
            report(&o, seed);
            let _ = o.recording.save(&dir.join(format!("run-{seed}.json")));
            if o.goal {
                // **Every winning seed's exit state is kept, not just the
                // first.** A link's entry tolerance is the interesting
                // question — how far off nominal can it start and still work?
                // — and it cannot be asked at all from a single state.
                let _ = std::fs::write(
                    states.join(format!("{link:02}-s{seed}.gbss")),
                    &o.state,
                );
                if solved.is_none() {
                    solved = Some(o.state.clone());
                    let _ =
                        std::fs::write(states.join(format!("{link:02}.gbss")), &o.state);
                }
            }
        }
        match solved {
            Some(st) => {
                println!("  link {link} solved; state carried forward");
                carried = Some(st);
            }
            None => {
                eprintln!(
                    "  link {link} was NOT solved in {} attempt(s) — the chain stops \
                     here rather than shortcutting past it",
                    a.seeds
                );
                exit(1);
            }
        }
    }
}

/// Many divergent attempts at one link, successes and failures both kept.
///
/// **Fanned out across threads, and determinism is not traded for it.** A run
/// is a function of (start state, scenario, seed) and nothing else — the
/// driver's randomness is a seeded xorshift, the emulator is deterministic, and
/// no two workers share anything but the read-only cartridge bytes. So the
/// same seed produces the same recording whatever the worker count, which
/// `--verify-determinism` checks rather than assumes.
///
/// Each worker gets its own directory because the MBC writes a `.sav` beside
/// the cartridge it was handed.
fn cmd_corpus(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let sp = a.scenario.clone().unwrap_or_else(|| {
        eprintln!("--scenario is required");
        exit(2);
    });
    let name = sp.file_stem().unwrap().to_string_lossy().to_string();
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/corpus").join(&name));
    std::fs::create_dir_all(&out).expect("output directory");
    // A pool of entry states rather than one, when the caller has them. Every
    // attempt takes `seed % pool.len()`, so which state a seed used is
    // recoverable from the seed alone and the corpus stays reproducible.
    // Shared, never copied per worker: the pool is read-only and a link-04
    // pool is 85 MB, so cloning it into sixteen workers was 1.4 GB of memcpy
    // and peak RSS to match. Same correction as `run_arm`'s.
    let pool = std::sync::Arc::new(entry_pool(a));
    let workers = a.workers.max(1);
    println!(
        "entry states: {}",
        if pool.len() > 1 {
            format!("{} of them, one per seed modulo the pool", pool.len())
        } else {
            "1".into()
        }
    );
    println!(
        "{}: {} attempt(s) from seed {}, {workers} worker(s)",
        name, a.runs, a.seed0
    );
    let t = std::time::Instant::now();
    let next = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let results =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u64, String, u64)>::new()));
    // Totals for the throughput line. Emulated frames, and how many of the
    // milliseconds spent on them were inside the emulator rather than around
    // it — a swarm figure that does not separate the two cannot say which half
    // to work on.
    let tot_frames = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let tot_emu_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let tot_wall_ms = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let spread = a.idle_spread;
    let exits = a.exits;
    let exit_dir = out.join("exits");
    if exits > 0 {
        std::fs::create_dir_all(&exit_dir).expect("exits directory");
    }
    let peak = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let live = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));

    std::thread::scope(|sc| {
        for w in 0..workers {
            let (next, results, peak, live) =
                (next.clone(), results.clone(), peak.clone(), live.clone());
            let (tot_frames, tot_emu_ms, tot_wall_ms) =
                (tot_frames.clone(), tot_emu_ms.clone(), tot_wall_ms.clone());
            let (rom, sp, out, pool) =
                (rom.clone(), sp.clone(), out.clone(), pool.clone());
            let exit_dir = exit_dir.clone();
            let runs = a.runs;
            let seed0 = a.seed0;
            let primitive = a.primitive;
            let paths = a.paths;
            sc.spawn(move || {
                use std::sync::atomic::Ordering::Relaxed;
                let dir = out.join(format!("w{w}"));
                let n = live.fetch_add(1, Relaxed) + 1;
                peak.fetch_max(n, Relaxed);
                loop {
                    let k = next.fetch_add(1, Relaxed);
                    if k >= runs {
                        break;
                    }
                    let seed = seed0 + k as u64;
                    let entry = pool[(seed as usize) % pool.len()].clone();
                    // The three independence axes, one expression each: which
                    // held-out entry state, which driver seed, and how far
                    // into the world the attempt starts.
                    let idle = phase_for(seed, spread);
                    let o =
                        play(&rom, &sp, entry, seed, &dir, true, primitive, paths, idle);
                    // The next link's entry pool. Only successful exits: a
                    // pool seeded from failures would certify the following
                    // link against worlds no real run ever reaches, which is
                    // the whole defect the chain figure exists to expose.
                    if o.goal && exits > 0 && seed < seed0 + exits as u64 {
                        let _ = std::fs::write(
                            exit_dir.join(format!("{seed:05}.gbss")),
                            &o.state,
                        );
                    }
                    let file = format!("run-{seed:05}.json");
                    let _ = o.recording.save(&out.join(&file));
                    if paths {
                        write_path(&out.join(format!("path-{seed:05}.csv")), &o.path);
                    }
                    let traj = trajectory_hash(&o);
                    let row = format!(
                        "{{\"seed\":{seed},\"file\":\"{file}\",\"goal\":{},\
                         \"ending\":\"{}\",\"frames\":{},\"decisions\":{},\
                         \"wall_ms\":{},\"map\":{},\"x\":{},\"y\":{},\
                         \"idle\":{idle},\"entry\":{},\
                         \"trajectory\":\"{traj:016x}\",\
                         \"final_state\":\"{:016x}\"}}",
                        o.goal,
                        o.ending,
                        o.frames,
                        o.decisions,
                        o.wall_ms,
                        o.map,
                        o.x,
                        o.y,
                        (seed as usize) % pool.len(),
                        o.recording.final_state
                    );
                    tot_frames.fetch_add(o.frames_total, Relaxed);
                    tot_emu_ms.fetch_add(o.emulate_ms as u64, Relaxed);
                    tot_wall_ms.fetch_add(o.wall_ms as u64, Relaxed);
                    results.lock().unwrap().push((seed, row, traj));
                }
                live.fetch_sub(1, Relaxed);
            });
        }
    });

    let mut rows = results.lock().unwrap().clone();
    rows.sort_by_key(|(s, _, _)| *s);
    let goals = rows
        .iter()
        .filter(|(_, r, _)| r.contains("\"goal\":true"))
        .count();
    let mut index = String::new();
    for (_, r, _) in &rows {
        index.push_str(r);
        index.push('\n');
    }
    std::fs::write(out.join("index.ndjson"), &index).expect("write the index");

    let secs = t.elapsed().as_millis() as f64 / 1000.0;
    // The rule of three: no failures in N attempts bounds the failure rate at
    // roughly 3/N, so a "100%" without an N beside it is not a measurement.
    let bound = if goals as u32 == a.runs {
        format!(
            " — zero failures in {} bounds the failure rate at about {:.2}% \
             (rule of three); {} attempts would be needed for a 99.9% claim",
            a.runs,
            300.0 / a.runs as f64,
            3000
        )
    } else {
        String::new()
    };
    println!(
        "{goals} of {} reached the goal ({:.1}%){bound}",
        a.runs,
        goals as f64 * 100.0 / a.runs as f64
    );
    println!(
        "{:.1} s wall, {:.2} episodes/second, peak concurrency {}",
        secs,
        a.runs as f64 / secs.max(0.001),
        peak.load(std::sync::atomic::Ordering::Relaxed)
    );
    // **The swarm's own unit.** Episodes per second depends on how long an
    // episode is and so cannot be compared between links or against anybody
    // else's harness; emulated frames per second of wall clock can. The
    // emulation share says which half of the loop the next hour belongs to.
    {
        use std::sync::atomic::Ordering::Relaxed;
        let frames = tot_frames.load(Relaxed);
        let emu = tot_emu_ms.load(Relaxed) as f64;
        let wall = tot_wall_ms.load(Relaxed) as f64;
        println!(
            "{:.0} emulated frames/s aggregate ({} frames); {:.1} s emulating, \
             {:.1} s in the run loop, {:.1} s of worker time outside it",
            frames as f64 / secs.max(0.001),
            frames,
            emu / 1000.0,
            wall / 1000.0,
            (secs * workers as f64) - wall / 1000.0
        );
    }
    // How the battles inside those episodes went. A link can pass while the
    // battle policy is quietly awful; see `docs/agent/battle-policy.md`.
    if let Some(b) = tally::report() {
        println!("{b}");
    }
    println!("{}", out.join("index.ndjson").display());
    let dupes = report_independence(&rows, pool.len(), spread);
    if dupes > 0 {
        exit(3);
    }
}

/// Refuse to call a corpus a measurement until its attempts are distinct.
///
/// **This guard is worth more than the bug that prompted it.** A certification
/// loop that varies nothing the game can see produces N copies of one attempt
/// and reports them as N successes, and every summary downstream — the pass
/// rate, the rule-of-three bound, the end-to-end chain figure — is then a
/// statement about a sample of one wearing a sample of N's clothes. It is
/// invisible in the verdict: a duplicated corpus and an honest one both print
/// "3000 of 3000". Only the trajectories can tell them apart, so they are
/// counted here, on every run, and a collision is a non-zero exit rather than
/// a line of prose somebody might not read.
///
/// Returns the number of attempts that duplicated an earlier one.
fn report_independence(rows: &[(u64, String, u64)], pool: usize, spread: u32) -> usize {
    let mut seen: HashMap<u64, u64> = HashMap::new();
    let mut collisions: Vec<(u64, u64)> = Vec::new();
    for (seed, _, traj) in rows {
        match seen.get(traj) {
            Some(first) => collisions.push((*first, *seed)),
            None => {
                seen.insert(*traj, *seed);
            }
        }
    }
    println!(
        "independence: {} distinct trajectories in {} attempts, \
         {pool} entry state(s), phase spread {spread}",
        seen.len(),
        rows.len()
    );
    if collisions.is_empty() {
        return 0;
    }
    eprintln!(
        "\nINDEPENDENCE FAILURE — {} of {} attempts repeat an earlier one \
         exactly.\n\
         This is NOT a measurement of {} attempts. Emulation here is a pure \
         function of (start state, scenario, seed), so identical trajectories \
         mean the attempts differed in nothing the cartridge could see.\n\
         Vary all three axes: --start-dir for the entry pool, distinct --seed0 \
         offsets, and --idle-spread for the phase (0 disables it).",
        collisions.len(),
        rows.len(),
        rows.len()
    );
    for (first, dup) in collisions.iter().take(5) {
        eprintln!("  seed {dup} played exactly the same game as seed {first}");
    }
    if collisions.len() > 5 {
        eprintln!("  ... and {} more", collisions.len() - 5);
    }
    collisions.len()
}

/// Same seed, same answer, whatever the worker count.
///
/// **This is the property the whole corpus rests on**, so it is checked rather
/// than assumed: the same three seeds are played once serially and once across
/// the workers, and every final-state hash must agree.
fn cmd_verify(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let sp = a.scenario.clone().unwrap_or_else(|| {
        eprintln!("--scenario is required");
        exit(2);
    });
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/verify"));
    let start = a
        .start
        .as_ref()
        .map(|p| std::fs::read(p).expect("start state"));
    let seeds: Vec<u64> = (0..a.runs.max(3) as u64).map(|k| a.seed0 + k).collect();

    let serial: Vec<(u64, u64, u64)> = seeds
        .iter()
        .map(|&seed| {
            let o = play(
                &rom,
                &sp,
                start.clone(),
                seed,
                &out.join("s"),
                true,
                a.primitive,
                false,
                phase_for(seed, a.idle_spread),
            );
            (seed, o.recording.final_state, o.frames)
        })
        .collect();

    let par = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    std::thread::scope(|sc| {
        for (i, &seed) in seeds.iter().enumerate() {
            let (rom, sp, out, start, par) = (
                rom.clone(),
                sp.clone(),
                out.clone(),
                start.clone(),
                par.clone(),
            );
            let primitive = a.primitive;
            let spread = a.idle_spread;
            sc.spawn(move || {
                let o = play(
                    &rom,
                    &sp,
                    start,
                    seed,
                    &out.join(format!("p{i}")),
                    true,
                    primitive,
                    false,
                    phase_for(seed, spread),
                );
                par.lock()
                    .unwrap()
                    .push((seed, o.recording.final_state, o.frames));
            });
        }
    });
    let mut par = par.lock().unwrap().clone();
    par.sort_by_key(|r| r.0);

    let mut bad = 0;
    for (s, p) in serial.iter().zip(par.iter()) {
        let ok = s == p;
        if !ok {
            bad += 1;
        }
        println!(
            "seed {:5}  serial {:016x}/{:6} frames   parallel {:016x}/{:6}   {}",
            s.0,
            s.1,
            s.2,
            p.1,
            p.2,
            if ok { "same" } else { "DIFFERENT" }
        );
    }
    if bad == 0 {
        println!(
            "\nDETERMINISTIC — {} seed(s) identical serially and in parallel",
            seeds.len()
        );
    } else {
        eprintln!("\n{bad} seed(s) DIFFER. Determinism is broken; nothing else matters.");
        exit(1);
    }
}

/// The player's own overworld sprite, read out of the running machine.
///
/// **Not drawn, not vendored, not guessed.** Gen 1 puts the player in OAM
/// slots 0-3 as four 8x8 tiles around screen (64,64); the tile numbers are in
/// OAM, the pixels are in VRAM at `$8000 + 16 * tile`, and the sprite palette
/// is `OBP0` at `$FF48`. Colour index 0 of a sprite is transparent, which is
/// what makes the cut-out exact rather than a colour key — the same reasoning
/// the trainer panel's flood fill uses for a Pokémon.
///
/// The facing byte is `wSpritePlayerStateData1FacingDirection` ($C109): 0 down,
/// 4 up, 8 left, 12 right. Right is the mirror of left, which is why the game
/// stores three and flips one, and why this writes what it finds rather than
/// assuming four distinct pictures exist.
fn cmd_sprites(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/sprites"));
    std::fs::create_dir_all(&out).expect("output directory");
    // No save path: this reads OAM and VRAM out of a supplied state and
    // presses four buttons. Nothing here wants a battery beside the player's
    // own cartridge, and nothing reads one.
    let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let start = a.start.as_ref().unwrap_or_else(|| {
        eprintln!("sprites needs --start FILE (a state standing in the overworld)");
        exit(2);
    });
    let bytes = std::fs::read(start).expect("start state");
    gb.load_state(&bytes).expect("load the start state");

    // Face each way in turn and grab what the machine draws. A press in a
    // direction you are not facing only turns you, which is exactly what is
    // wanted here.
    for (name, key) in [
        ("down", terminalgb_plugin_api::KeypadKey::Down),
        ("up", terminalgb_plugin_api::KeypadKey::Up),
        ("left", terminalgb_plugin_api::KeypadKey::Left),
        ("right", terminalgb_plugin_api::KeypadKey::Right),
    ] {
        // Two taps: a press in a direction you are not facing only turns
        // you, and the second one is what makes the turn stick when the tile
        // ahead is walkable and the first press stepped instead.
        for _ in 0..2 {
            gb.queue_press(key, 16, 8);
        }
        for _ in 0..60 {
            gb.frame();
        }
        let (facing, px) = gb.with_machine(Cap::OBSERVE | Cap::READ_MEMORY, |m| {
            (m.peek(0xC109).unwrap_or(0), player_sprite(m))
        });
        let path = out.join(format!("player-{name}-f{facing}.png"));
        write_rgba(&path, 16, 16, &px);
        println!("{} facing {facing} -> {}", name, path.display());
    }
}

/// OAM slots 0-3, laid out as a 16x16 RGBA cut-out with colour 0 transparent.
fn player_sprite(m: &mut terminalgb_plugin_api::Machine) -> Vec<u8> {
    let obp0 = m.peek(0xFF48).unwrap_or(0xE4);
    let shade = |i: u8| -> u8 {
        match (obp0 >> (2 * i)) & 3 {
            0 => 255,
            1 => 192,
            2 => 96,
            _ => 0,
        }
    };
    let mut out = vec![0u8; 16 * 16 * 4];
    // The four slots' screen origin is the minimum of their own coordinates,
    // so the cut-out is aligned however the game happens to have placed them.
    let mut ent = Vec::new();
    for slot in 0..4u16 {
        let b = 0xFE00 + slot * 4;
        ent.push((
            m.peek(b).unwrap_or(0) as i32 - 16,
            m.peek(b + 1).unwrap_or(0) as i32 - 8,
            m.peek(b + 2).unwrap_or(0),
            m.peek(b + 3).unwrap_or(0),
        ));
    }
    let oy = ent.iter().map(|e| e.0).min().unwrap_or(0);
    let ox = ent.iter().map(|e| e.1).min().unwrap_or(0);
    for (sy, sx, tile, attr) in ent {
        let base = 0x8000 + 16 * tile as u16;
        for row in 0..8u16 {
            let lo = m.peek(base + 2 * row).unwrap_or(0);
            let hi = m.peek(base + 2 * row + 1).unwrap_or(0);
            for col in 0..8u16 {
                let bit = 7 - col;
                let c = ((lo >> bit) & 1) | (((hi >> bit) & 1) << 1);
                if c == 0 {
                    continue; // transparent, by the hardware's own rule
                }
                let (mut dx, mut dy) = (col as i32, row as i32);
                if attr & 0x20 != 0 {
                    dx = 7 - dx;
                }
                if attr & 0x40 != 0 {
                    dy = 7 - dy;
                }
                let (px, py) = (sx - ox + dx, sy - oy + dy);
                if !(0..16).contains(&px) || !(0..16).contains(&py) {
                    continue;
                }
                let g = shade(c);
                let o = ((py * 16 + px) * 4) as usize;
                out[o] = g;
                out[o + 1] = g;
                out[o + 2] = g;
                out[o + 3] = 255;
            }
        }
    }
    out
}

/// A minimal RGBA PNG, using the encoder the terminal transport already has.
fn write_rgba(path: &Path, w: usize, h: usize, rgba: &[u8]) {
    let mut png = Vec::new();
    terminalgb::termimage::png_into(rgba, w as u32, h as u32, &mut png);
    let _ = std::fs::write(path, png);
}

/// One agent plays the whole chain, and its track is continuous.
///
/// **Not one film per link.** A link is eight steps of a bedroom; a chain is
/// Pallet to Pewter, and only the second is worth watching. Each agent carries
/// its *own* exit state from link to link rather than a shared one, so the
/// agents diverge for the same reason a swarm does and the divergence is the
/// picture.
/// A scenario path's link name, for a one-line report.
fn link_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default()
}

fn cmd_film(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/film"));
    std::fs::create_dir_all(&out).expect("output directory");
    // **`--from`/`--to`, because the chain outgrew one film.** This used to
    // take every link there was, which was right while the chain ended at
    // Pewter and is not now: the gym arc is its own cut, deliberately, and a
    // command that always plays everything cannot produce either of them on
    // purpose. It also means the published 01-14 film stays reproducible after
    // links 15-19 landed — see `docs/agent/brock.md` §14 for why they are not
    // sewn together.
    let all = chain_files();
    // Carry the link's number in the whole chain, not its position in the
    // slice: a span file saying "link 1" for `15-train-for-brock` is a span
    // file nobody can line up against a report.
    let files: Vec<(usize, String)> = all
        .iter()
        .enumerate()
        .map(|(i, f)| (i + 1, f.clone()))
        .filter(|(link, _)| *link >= a.from && *link <= a.to)
        .collect();
    if files.is_empty() {
        eprintln!("--from {} --to {} selects no link", a.from, a.to);
        exit(2);
    }
    let agents = a.runs.max(1);
    let workers = a.workers.max(1);
    // **A slice that does not start at link 1 needs somewhere to start.**
    // Link 1 boots the cartridge cold; every link after it begins from the
    // previous one's exit state, so `--from 15` without a pool would silently
    // film fifteen agents standing on the Nintendo logo. `--start-dir` gives
    // each agent `ag % pool.len()`, the same rule `corpus` uses, so which
    // state an agent began from is recoverable from its number.
    let pool: Vec<Option<Vec<u8>>> = if a.from > 1 {
        if a.start_dir.is_none() && a.start.is_none() {
            eprintln!(
                "--from {} starts part way through the chain, so it needs \
                 --start-dir (a pool of entry states) or --start (one)",
                a.from
            );
            exit(2);
        }
        entry_pool(a)
    } else {
        vec![None]
    };
    let pool = std::sync::Arc::new(pool);
    println!(
        "{agents} agent(s) over {} of {} link(s) ({}..{}), {workers} worker(s), \
         {} entry state(s)",
        files.len(),
        all.len(),
        files.first().map(|(_, f)| link_name(f)).unwrap_or_default(),
        files.last().map(|(_, f)| link_name(f)).unwrap_or_default(),
        pool.len(),
    );

    let t = std::time::Instant::now();
    let next = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let done =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, usize, u64)>::new()));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            let (next, done) = (next.clone(), done.clone());
            let (rom, out, files) = (rom.clone(), out.clone(), files.clone());
            let pool = pool.clone();
            let primitive = a.primitive;
            let spread = a.idle_spread;
            sc.spawn(move || loop {
                let ag = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if ag >= agents {
                    break;
                }
                let seed = 1 + ag as u64;
                let dir = out.join(format!("w{ag}"));
                let mut carried: Option<Vec<u8>> = pool[ag as usize % pool.len()].clone();
                let mut track: Vec<terminalgb::scenario::run::PathPoint> = Vec::new();
                let mut base = 0u32;
                let mut reached = 0usize;
                // Where each link began and ended in the joined stream, which
                // is what `swarm_film.py` draws its per-link bars and its
                // stage timeline from. `chain_tracks.py` writes the same file
                // when a film is stitched from separate corpus runs; writing
                // it here means `film` is a complete source on its own.
                let mut spans: Vec<String> = Vec::new();
                for (n, (link, f)) in files.iter().enumerate() {
                    let sp = PathBuf::from(f);
                    let o = play(
                        &rom,
                        &sp,
                        carried.clone(),
                        seed,
                        &dir,
                        true,
                        primitive,
                        true,
                        phase_for(seed, spread),
                    );
                    for mut p in o.path {
                        p.frame += base;
                        track.push(p);
                    }
                    let start = base;
                    base = track.last().map(|p| p.frame).unwrap_or(base);
                    spans.push(format!(
                        "{},{},{},{},{}",
                        link,
                        link_name(f),
                        start,
                        base,
                        o.goal
                    ));
                    if !o.goal {
                        break;
                    }
                    reached = n + 1;
                    carried = Some(o.state);
                }
                write_path(&out.join(format!("agent-{ag:02}.csv")), &track);
                let _ = std::fs::write(
                    out.join(format!("agent-{ag:02}.links.csv")),
                    format!("link,name,start,end,goal\n{}\n", spans.join("\n")),
                );
                let _ = std::fs::remove_dir_all(&dir);
                done.lock().unwrap().push((ag, reached, base as u64));
            });
        }
    });
    let mut rows = done.lock().unwrap().clone();
    rows.sort();
    for (ag, reached, frames) in &rows {
        println!(
            "agent {ag:2} reached link {reached:2} of {} ({frames} frames)",
            files.len()
        );
    }
    let full = rows.iter().filter(|r| r.1 == files.len()).count();
    println!(
        "{full} of {agents} agent(s) finished the chain; {:.1} s wall",
        t.elapsed().as_millis() as f64 / 1000.0
    );
}

/// What a pool of entry states actually holds.
///
/// **A link's precondition is a claim about the pool it starts from**, and
/// until this existed the only way to check one was to run the link and read
/// the failures. The Brock links needed it immediately: "link 11 trains the
/// lead to level 10" is a floor, and what the gym cares about is the
/// *distribution* — which is a different question and has a different answer
/// per starter.
///
/// No frames are emulated. A save state carries the party, so this is one
/// `load_state` and a handful of bus reads per file, exactly as
/// [`lead_species`] already does.
fn cmd_states(a: &Args) {
    use terminalgb::longplay::watch::{Watch, Watches};
    let rom = load_cart(&need_rom(a));
    let dir = a.start_dir.clone().unwrap_or_else(|| {
        eprintln!("--start-dir is required");
        exit(2);
    });
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", dir.display());
            exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "gbss").unwrap_or(false))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("no .gbss files in {}", dir.display());
        exit(1);
    }
    let names = [
        "lead_species",
        "lead_level",
        "lead_hp",
        "lead_max_hp",
        "lead_move1",
        "lead_move2",
        "lead_move3",
        "lead_move4",
    ];
    let mut list: Vec<Watch> = names.iter().filter_map(|n| Watch::named(n)).collect();
    // Raw addresses, so a byte a new goal is about to rest on can be read out
    // of real states *before* the goal is written. `NAME@0xD719` or `0xD719`.
    let mut peeked: Vec<String> = Vec::new();
    for spec in &a.peek {
        let (nm, addr) = match spec.split_once('@') {
            Some((n, addr)) => (n.to_string(), addr),
            None => (spec.clone(), spec.as_str()),
        };
        let hex = addr
            .trim_start_matches("0x")
            .trim_start_matches("0X")
            .trim_start_matches('$');
        let Ok(a16) = u16::from_str_radix(hex, 16) else {
            eprintln!("--peek {spec:?}: not an address");
            exit(2);
        };
        let text = format!("{{\"name\":\"{nm}\",\"addr\":\"{a16:#06X}\",\"len\":1}}");
        let j = match terminalgb_plugin_agent::json::parse(&text) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("--peek {spec:?}: {}", e.0);
                exit(2);
            }
        };
        match Watch::from_json(&j) {
            Ok(watch) => {
                peeked.push(nm);
                list.push(watch);
            }
            Err(e) => {
                eprintln!("--peek {spec:?}: {e}");
                exit(2);
            }
        }
    }
    let w = Watches(list);
    // Per (species, level): how many, and the worst and best HP fraction.
    let mut byspecies: HashMap<(u8, u64), u32> = HashMap::new();
    let mut unreadable = 0u32;
    println!(
        "{:>5}  {:<11} {:>3}  {:>7}  {:>6} {:>6}  moves",
        "n", "lead", "lv", "hp", "money", "badges"
    );
    for (i, f) in files.iter().enumerate() {
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("  {}: {e}", f.display());
                unreadable += 1;
                continue;
            }
        };
        let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
        if gb.load_state(&bytes).is_err() {
            eprintln!("  {}: not a state for this cartridge", f.display());
            unreadable += 1;
            continue;
        }
        let sample = gb.with_machine(Cap::READ_MEMORY, |m| w.read(m));
        let prog = gb.with_machine(Cap::READ_MEMORY, terminalgb::longplay::read_progress);
        let sp = sample.get("lead_species").unwrap_or(0) as u8;
        let lv = sample.get("lead_level").unwrap_or(0);
        let hp = sample.get("lead_hp").unwrap_or(0);
        let max = sample.get("lead_max_hp").unwrap_or(0);
        let extra: Vec<String> = peeked
            .iter()
            .map(|n| format!("{n}={:#04X}", sample.get(n).unwrap_or(0)))
            .collect();
        let mv: Vec<String> = (1..=4)
            .filter_map(|k| sample.get(&format!("lead_move{k}")))
            .filter(|m| *m != 0)
            .map(|m| move_name(&rom, m as u8))
            .collect();
        *byspecies.entry((sp, lv as u64)).or_default() += 1;
        if i < 12 || files.len() <= 24 {
            println!(
                "{:>5}  {:<11} {:>3}  {:>3}/{:<3}  {:>6} {:>6}  {}",
                i,
                species_name(&rom, sp),
                lv,
                hp,
                max,
                prog.money,
                prog.badges.count_ones(),
                if extra.is_empty() {
                    mv.join(", ")
                } else {
                    format!("{}  {}", extra.join(" "), mv.join(", "))
                }
            );
        } else if i == 12 {
            println!("      … {} more, summarised below", files.len() - 12);
        }
    }
    println!("\n{} state(s), {unreadable} unreadable", files.len());
    let mut keys: Vec<_> = byspecies.keys().copied().collect();
    keys.sort();
    let mut species_seen: Vec<u8> = keys.iter().map(|(s, _)| *s).collect();
    species_seen.dedup();
    for sp in species_seen {
        let rows: Vec<_> = keys.iter().filter(|(s, _)| *s == sp).collect();
        let total: u32 = rows.iter().map(|k| byspecies[k]).sum();
        let lo = rows.first().map(|k| k.1).unwrap_or(0);
        let hi = rows.last().map(|k| k.1).unwrap_or(0);
        print!(
            "  {:<11} {:>4} state(s), level {lo}-{hi}:",
            species_name(&rom, sp),
            total
        );
        for k in &rows {
            print!(" L{}x{}", k.1, byspecies[k]);
        }
        println!();
    }
}

/// A move's name from the cartridge, never a table written down here — the
/// anti-drift rule `AGENTS.md` records for every Gen 1 constant.
#[cfg(feature = "plugin-trainer")]
fn move_name(rom: &Cart, index: u8) -> String {
    let n = terminalgb_plugin_trainer::rom::RomTables::new(rom.0.as_ref().clone())
        .move_name(index);
    if n.is_empty() {
        format!("#{index}")
    } else {
        n
    }
}

#[cfg(not(feature = "plugin-trainer"))]
fn move_name(_rom: &Cart, index: u8) -> String {
    format!("#{index}")
}

/// Print every route the solver will walk, as JSON, for
/// `tools/pewter-chain/route_audit.py`.
///
/// **The audit has to read the routes the solver actually uses**, not a copy of
/// them scraped out of this file: a regex over `const X: &[(u8, u8)]` misses
/// anything built at run time, and a route the auditor cannot see is exactly
/// the route that will contain the next trap. Both the seeded variants (link
/// 05's four approaches) and the heal detour are included, and each is labelled
/// with the seed it belongs to so a failure names something reproducible.
fn cmd_routes() {
    let mut out = String::from("[\n");
    let mut first = true;
    let mut emit =
        |scenario: &str, seed: u64, kind: &str, plans: &HashMap<u8, MapPlan>| {
            let mut maps: Vec<_> = plans.iter().collect();
            maps.sort_by_key(|(m, _)| **m);
            for (map, stages) in maps {
                for (i, st) in stages.iter().enumerate() {
                    if !first {
                        out.push_str(",\n");
                    }
                    first = false;
                    let via: Vec<String> =
                        st.via.iter().map(|(x, y)| format!("[{x},{y}]")).collect();
                    let finish = match st.finish {
                        Finish::Arrive => "arrive".to_string(),
                        Finish::Step(d) => format!("step {d}"),
                        Finish::Talk(d) => format!("talk {d}"),
                        Finish::Grind(a, b) => format!("grind {a:?} {b:?}"),
                    };
                    out.push_str(&format!(
                    "  {{\"scenario\":\"{scenario}\",\"seed\":{seed},\"kind\":\"{kind}\",\
                     \"map\":{map},\"stage\":{i},\"finish\":\"{finish}\",\"via\":[{}]}}",
                    via.join(",")
                ));
                }
            }
        };
    for f in chain_files() {
        let name = Path::new(&f)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        // Link 05 picks its approach from the seed; every other leg is the
        // same route for all of them, and emitting four copies would only
        // make the audit slower.
        for seed in 0..4u64 {
            let r = route_for(&name, seed);
            let key = |p: &HashMap<u8, MapPlan>| {
                let mut v: Vec<_> = p.iter().map(|(m, s)| (*m, s.clone())).collect();
                v.sort_by_key(|(m, _)| *m);
                format!("{v:?}")
            };
            if seed > 0 && key(&r) == key(&route_for(&name, 0)) {
                continue;
            }
            emit(&name, seed, "route", &r);
        }
        let h = heal_route(&name);
        if !h.is_empty() {
            emit(&name, 0, "heal", &h);
        }
    }
    out.push_str("\n]\n");
    println!("{out}");
}

fn chain_files() -> Vec<String> {
    let dir = Path::new("scenarios/pewter");
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| {
            eprintln!("cannot read {}: {e}", dir.display());
            exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path().to_string_lossy().to_string())
        .filter(|p| p.ends_with(".json"))
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------------------
// The A/B test rig
// ---------------------------------------------------------------------------

/// Read the lead's **internal** species index out of a saved state.
///
/// One machine, one `load_state`, four bus reads: the state carries the party,
/// so no frames need to be emulated to find out what is in it. Zero means the
/// state has no party — which is a legitimate answer for a link before the
/// starter, and is why a subject that asks for a species simply finds no
/// entries rather than failing.
fn lead_species(rom: &Cart, state: &[u8]) -> u8 {
    use terminalgb::longplay::watch::{Watch, Watches};
    let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
    if gb.load_state(state).is_err() {
        return 0;
    }
    let w = Watches(
        ["lead_species"]
            .iter()
            .filter_map(|n| Watch::named(n))
            .collect(),
    );
    gb.with_machine(Cap::READ_MEMORY, |m| w.read(m))
        .get("lead_species")
        .unwrap_or(0) as u8
}

/// The four moves the lead is holding in a saved state, in slot order.
///
/// The same one-machine, no-frames read as [`lead_species`]: the state carries
/// the party, so what a player chose at a prompt is legible from the state it
/// left behind without replaying anything.
fn lead_moves(rom: &Cart, state: &[u8]) -> [u8; 4] {
    use terminalgb::longplay::watch::{Watch, Watches};
    let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
    if gb.load_state(state).is_err() {
        return [0; 4];
    }
    let w = Watches(
        ["lead_move1", "lead_move2", "lead_move3", "lead_move4"]
            .iter()
            .filter_map(|n| Watch::named(n))
            .collect(),
    );
    let sample = gb.with_machine(Cap::READ_MEMORY, |m| w.read(m));
    let mut out = [0u8; 4];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = sample.get(&format!("lead_move{}", i + 1)).unwrap_or(0) as u8;
    }
    out
}

/// A species name from the cartridge's own `MonsterNames`, when the trainer
/// plugin is compiled in. **Never a table written down here** — the anti-drift
/// rule in `AGENTS.md` exists because nine files once held their own copy of
/// `wCurMap` and nothing could say whether they agreed.
#[cfg(feature = "plugin-trainer")]
fn species_name(rom: &Cart, index: u8) -> String {
    let n = terminalgb_plugin_trainer::rom::RomTables::new(rom.0.as_ref().clone())
        .species_name(index);
    if n.is_empty() {
        format!("#{index}")
    } else {
        n
    }
}

#[cfg(not(feature = "plugin-trainer"))]
fn species_name(_rom: &Cart, index: u8) -> String {
    format!("#{index}")
}

/// Resolve a `--lead` argument: a decimal or `0x` index, or a species name
/// looked up in the cartridge.
fn lead_arg(rom: &Cart, s: &str) -> Option<u8> {
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return u8::from_str_radix(hex, 16).ok();
    }
    if let Ok(n) = s.parse::<u8>() {
        return Some(n);
    }
    let want = s.to_ascii_uppercase();
    (1u8..=190).find(|&i| species_name(rom, i).to_ascii_uppercase() == want)
}

/// How an arm makes its driver, given the scenario's name.
///
/// A factory rather than a driver because the expert wants the scenario name
/// and nothing else does, and because a fresh driver is built per attempt.
type DriverFactory = Box<dyn Fn(&str) -> Box<dyn Driver>>;

/// The driver for a non-expert arm. The expert needs the scenario name, so it
/// is built at the call site instead.
fn make_driver(p: &abrig::Player, seed: u64) -> Box<dyn Driver> {
    match p {
        abrig::Player::Random { seed_base } => Box::new(
            terminalgb_plugin_agent::driver::RandomDriver::new(seed_base ^ seed),
        ) as Box<dyn Driver>,
        abrig::Player::Command { program, args } => Box::new(
            terminalgb_plugin_agent::subprocess::CommandDriver::new(
                program,
                args.clone(),
            )
            .with_name("student"),
        ) as Box<dyn Driver>,
        abrig::Player::Expert => {
            unreachable!("the expert arm is built where the scenario name is known")
        }
    }
}

fn cmd_ab(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let plan = build_plan(a, &rom);
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/testrig").join(&plan.name));
    std::fs::create_dir_all(&out).expect("output directory");

    // --- the pool, once, and split by subject ------------------------------
    let mut files: Vec<PathBuf> = std::fs::read_dir(&plan.pool)
        .unwrap_or_else(|e| {
            eprintln!("cannot read the entry pool {}: {e}", plan.pool.display());
            exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "gbss").unwrap_or(false))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("no .gbss files in {}", plan.pool.display());
        exit(1);
    }
    // Each entry, and — when the pool's producer supplied one — what the right
    // answer to its prompt is. See `abrig::Expectation`: a state with no file
    // beside it is simply not scored on that outcome.
    let mut annotated = 0usize;
    let states: std::sync::Arc<Vec<(Vec<u8>, Option<abrig::Expectation>)>> = files
        .into_iter()
        .map(|p| {
            let b = std::fs::read(&p).expect("entry state");
            let e = std::fs::read_to_string(p.with_extension("expect.json"))
                .ok()
                .map(|t| {
                    abrig::Expectation::parse(&t).unwrap_or_else(|e| {
                        eprintln!("{}: {e}", p.with_extension("expect.json").display());
                        exit(1);
                    })
                });
            if e.is_some() {
                annotated += 1;
            }
            (b, e)
        })
        .collect::<Vec<_>>()
        .into();
    if annotated > 0 {
        println!(
            "prompts    {annotated} of {} entry state(s) carry an expected answer",
            states.len()
        );
    }
    let mut subjects = plan.subjects.clone();
    if subjects.is_empty() {
        subjects.push(abrig::Subject {
            name: "all".into(),
            lead_species: Vec::new(),
        });
    }

    println!("{}", banner(&plan, &out));
    let mut cells: HashMap<(String, String), abrig::Cell> = HashMap::new();
    let mut pool_sizes: HashMap<String, usize> = HashMap::new();
    let workers = if plan.workers == 0 {
        default_workers()
    } else {
        plan.workers
    };

    // **Read the lead only when a subject asks to be filtered by one, and read
    // them across the workers when it does.** Each read is a whole machine —
    // the cartridge is cloned into it and a save state loaded — and a plan with
    // no `lead_species` anywhere never consults the answer, so on the ordinary
    // single-subject plan this is the difference between 600 machines and none.
    // When it *is* needed it is embarrassingly parallel and was serial.
    let leads: Vec<u8> = if subjects.iter().any(|s| !s.lead_species.is_empty()) {
        let out = std::sync::Mutex::new(vec![0u8; states.len()]);
        let next = std::sync::atomic::AtomicUsize::new(0);
        std::thread::scope(|sc| {
            for _ in 0..workers.min(states.len().max(1) as u32) {
                let (out, next, states, rom) = (&out, &next, &states, rom.clone());
                sc.spawn(move || {
                    use std::sync::atomic::Ordering::Relaxed;
                    loop {
                        let i = next.fetch_add(1, Relaxed);
                        if i >= states.len() {
                            break;
                        }
                        let v = lead_species(&rom, &states[i].0);
                        out.lock().unwrap()[i] = v;
                    }
                });
            }
        });
        out.into_inner().unwrap()
    } else {
        vec![0u8; states.len()]
    };

    // **Progress is on by default and silenced by a flag, never the reverse.**
    // Attempts that could actually run: a subject whose pool is empty is
    // skipped below, so it is not in the denominator.
    // Per subject, because `"runs": "pool"` gives each one as many attempts as
    // it has entry states and the subjects need not hold the same number.
    let runs_for = |sub: &abrig::Subject| -> u32 {
        if plan.runs_is_pool {
            leads.iter().filter(|l| sub.takes(**l)).count() as u32
        } else {
            plan.runs
        }
    };
    let live_total: u32 =
        subjects.iter().map(&runs_for).sum::<u32>() * plan.arms.len() as u32;
    let live = std::sync::Arc::new(abrig::Live::new(live_total));
    let began = std::time::Instant::now();
    let ticker = {
        let live = live.clone();
        let path = out.join("progress.ndjson");
        let quiet = a.quiet;
        std::thread::spawn(move || {
            use std::io::Write as _;
            use std::sync::atomic::Ordering::Relaxed;
            let mut f = std::fs::File::create(&path).ok();
            let emit = |live: &abrig::Live, f: &mut Option<std::fs::File>| {
                let (toon, json) = live.lines(began.elapsed().as_secs_f64());
                // **stdout, because an agent drives this through a shell and
                // does not read stderr** — the same correction `9d02819` made
                // to the other two agent-facing front ends. It is safe to put
                // it beside the report because it is TOON with a name of its
                // own (`progress[1]{…}`), so nothing can mistake a tick for a
                // row of the result. The same tick also goes to the file, so a
                // run can be followed after the fact.
                if !quiet {
                    println!("{toon}");
                    let _ = std::io::stdout().flush();
                }
                if let Some(f) = f.as_mut() {
                    let _ = writeln!(f, "{json}");
                    let _ = f.flush();
                }
            };
            let mut next = std::time::Instant::now() + PROGRESS_EVERY;
            while !live.stop.load(Relaxed) {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if std::time::Instant::now() >= next {
                    emit(&live, &mut f);
                    next += PROGRESS_EVERY;
                }
            }
            // One final tick, so the line and the file both end on the truth.
            emit(&live, &mut f);
        })
    };

    for sub in &subjects {
        // **A subject is a list of indices into the one shared pool, not a copy
        // of it.** The pool used to be cloned once per subject and then again
        // per worker: on the three-subject, three-arm, 700-state plan that is
        // nine cells x sixteen workers x 33 MB of save states, ~4.8 GB of
        // memcpy for data nobody mutates. `Arc` costs an atomic instead.
        let idx: std::sync::Arc<Vec<usize>> = std::sync::Arc::new(
            (0..states.len()).filter(|&i| sub.takes(leads[i])).collect(),
        );
        pool_sizes.insert(sub.name.clone(), idx.len());
        if idx.is_empty() {
            eprintln!(
                "subject {:?}: no entry state in the pool has that lead — skipped",
                sub.name
            );
            continue;
        }
        for arm in &plan.arms {
            let dir = out.join(format!("{}-{}", sub.name, arm.name));
            std::fs::create_dir_all(&dir).ok();
            let runs = runs_for(sub);
            live.begin(&format!("{}/{}", sub.name, arm.name), runs);
            let cell = run_arm(
                &rom,
                &plan,
                arm,
                &states,
                &idx,
                &dir,
                workers,
                runs,
                a.primitive,
                &live,
            );
            println!(
                "  {:<12} {:<10} {:>4}/{:<4} {}",
                sub.name,
                arm.name,
                cell.goals(),
                cell.n(),
                abrig::pct_ci(cell.goals(), cell.n())
            );
            cells.insert((sub.name.clone(), arm.name.clone()), cell);
        }
    }

    live.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = ticker.join();
    let text = render(&plan, &subjects, &cells, &pool_sizes)
        + &breakdown(
            &plan,
            &subjects,
            &cells,
            workers,
            began.elapsed().as_secs_f64(),
        );
    println!("\n{text}");
    std::fs::write(out.join("report.txt"), &text).ok();
    std::fs::write(
        out.join("report.json"),
        report_json(&plan, &subjects, &cells, &pool_sizes),
    )
    .ok();
    println!("{}", out.join("report.txt").display());
}

/// The plan, from a file if one was named and from flags otherwise. Flags
/// always win, so a committed plan can be re-run with one field changed
/// without editing it.
fn build_plan(a: &Args, rom: &Cart) -> abrig::Plan {
    let mut p = match &a.plan {
        Some(f) => {
            let text = std::fs::read_to_string(f).unwrap_or_else(|e| {
                eprintln!("cannot read {}: {e}", f.display());
                exit(1);
            });
            abrig::Plan::parse(&text).unwrap_or_else(|e| {
                eprintln!("{}: {e}", f.display());
                exit(1);
            })
        }
        None => abrig::Plan {
            name: "adhoc".into(),
            runs_is_pool: false,
            question: String::new(),
            split: abrig::Split::NotApplicable,
            scenario: PathBuf::new(),
            pool: PathBuf::new(),
            subjects: Vec::new(),
            arms: vec![
                abrig::ArmSpec {
                    name: "teacher".into(),
                    player: abrig::Player::Expert,
                },
                abrig::ArmSpec {
                    name: "random".into(),
                    player: abrig::Player::Random {
                        seed_base: 0x9E37_79B9,
                    },
                },
            ],
            runs: 200,
            seed0: 0,
            idle_spread: IDLE_SPREAD,
            workers: 0,
        },
    };
    if let Some(s) = &a.scenario {
        p.scenario = s.clone();
    }
    if let Some(d) = &a.start_dir {
        p.pool = d.clone();
    }
    if a.runs > 1 {
        p.runs = a.runs;
    }
    if a.seed0 != 0 {
        p.seed0 = a.seed0;
    }
    if a.idle_spread != IDLE_SPREAD {
        p.idle_spread = a.idle_spread;
    }
    if a.workers_given {
        p.workers = a.workers;
    }
    if let Some(s) = &a.split {
        p.split = abrig::Split::parse(s).unwrap_or_else(|| {
            eprintln!("--split must be seen, unseen or n/a");
            exit(2);
        });
    }
    if !a.leads.is_empty() {
        p.subjects = a
            .leads
            .iter()
            .map(|s| {
                let idx = lead_arg(rom, s).unwrap_or_else(|| {
                    eprintln!(
                        "--lead {s:?}: not a number and not a species in this cartridge"
                    );
                    exit(2);
                });
                abrig::Subject {
                    name: species_name(rom, idx),
                    lead_species: vec![idx],
                }
            })
            .collect();
    }
    if let Some(c) = &a.student {
        let mut it = c.split_whitespace();
        let program = it.next().unwrap_or("").to_string();
        let args: Vec<String> = it.map(|s| s.to_string()).collect();
        p.arms.push(abrig::ArmSpec {
            name: "student".into(),
            player: abrig::Player::Command { program, args },
        });
    }
    if p.scenario.as_os_str().is_empty() {
        eprintln!("--scenario (or a plan with one) is required");
        exit(2);
    }
    if p.pool.as_os_str().is_empty() {
        eprintln!("--start-dir (or a plan's entry_pool) is required");
        exit(2);
    }
    p
}

/// Play one arm over the whole subject pool, fanned out across workers.
#[allow(clippy::too_many_arguments)]
fn run_arm(
    rom: &Cart,
    plan: &abrig::Plan,
    arm: &abrig::ArmSpec,
    states: &std::sync::Arc<Vec<(Vec<u8>, Option<abrig::Expectation>)>>,
    idx: &std::sync::Arc<Vec<usize>>,
    dir: &Path,
    workers: u32,
    runs: u32,
    primitive: bool,
    live: &abrig::Live,
) -> abrig::Cell {
    let next = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let rows = std::sync::Arc::new(std::sync::Mutex::new(Vec::<abrig::Attempt>::new()));
    let tot = std::sync::Arc::new([
        std::sync::atomic::AtomicU64::new(0), // wall
        std::sync::atomic::AtomicU64::new(0), // emulate
        std::sync::atomic::AtomicU64::new(0), // setup
        std::sync::atomic::AtomicU64::new(0), // frames
    ]);
    std::thread::scope(|sc| {
        for w in 0..workers {
            let (next, rows, tot) = (next.clone(), rows.clone(), tot.clone());
            let (rom, sp) = (rom.clone(), plan.scenario.clone());
            let dir = dir.join(format!("w{w}"));
            let (seed0, spread) = (plan.seed0, plan.idle_spread);
            let player = arm.player.clone();
            // Shared, not copied: see the note where `idx` is built. The only
            // per-attempt clone left is the one entry state a worker is about
            // to load into a machine, which it consumes.
            let (states, idx) = (states.clone(), idx.clone());
            sc.spawn(move || {
                use std::sync::atomic::Ordering::Relaxed;
                loop {
                    let k = next.fetch_add(1, Relaxed);
                    if k >= runs {
                        break;
                    }
                    let seed = seed0 + k as u64;
                    // **The three axes, identical for every arm.** Which entry
                    // state, which driver seed, how far into the world the
                    // attempt starts — all a function of the seed alone, so
                    // two arms at seed N play the same fight.
                    let e = idx[(seed as usize) % idx.len()];
                    let entry = states[e].0.clone();
                    let idle = phase_for(seed, spread);
                    let mk: DriverFactory = match &player {
                        abrig::Player::Expert => Box::new(move |name: &str| {
                            Box::new(ChainDriver::new(name, seed).primitive(primitive))
                                as Box<dyn Driver>
                        }),
                        other => {
                            let other = other.clone();
                            Box::new(move |_: &str| make_driver(&other, seed))
                        }
                    };
                    // A subprocess arm may be a screen-only policy, so it is
                    // given a picture; the expert and the random baseline are
                    // not, and keep the pixel skip. Per worker, so two threads
                    // cannot overwrite each other's frame.
                    let frames = matches!(player, abrig::Player::Command { .. })
                        .then(|| dir.join("frames"));
                    let o = play_with(
                        &rom,
                        &sp,
                        Some(entry),
                        &dir,
                        true,
                        false,
                        idle,
                        mk.as_ref(),
                        frames.as_deref(),
                    );
                    let b = o.battles;
                    // What the player left the party holding, and whether the
                    // pool says that is right. One machine, no emulated frames.
                    let moves = lead_moves(&rom, &o.state);
                    let expect = states[e].1.as_ref();
                    let correct = expect.map(|x| x.satisfied_by(&moves));
                    let decline = expect.map(|x| x.decline).unwrap_or(false);
                    tot[0].fetch_add(o.wall_ms as u64, Relaxed);
                    tot[1].fetch_add(o.emulate_ms as u64, Relaxed);
                    tot[2].fetch_add(o.setup_ms as u64, Relaxed);
                    tot[3].fetch_add(o.frames_total, Relaxed);
                    live.tick();
                    rows.lock().unwrap().push(abrig::Attempt {
                        seed,
                        goal: o.goal,
                        ending: o.ending.clone(),
                        frames: o.frames,
                        decisions: o.decisions as u32,
                        battles: b.battles,
                        won: b.won,
                        fled: b.fled,
                        lost: b.lost,
                        frames_in_battle: b.frames_in_battle,
                        hp_permille: b.hp_left_permille,
                        hp_samples: b.hp_samples,
                        moves,
                        correct,
                        decline,
                    });
                }
            });
        }
    });
    let mut attempts = std::sync::Arc::try_unwrap(rows)
        .expect("workers are done")
        .into_inner()
        .unwrap();
    attempts.sort_by_key(|a| a.seed);
    // One row per attempt, beside the cell. **A summary cannot be debugged**:
    // when a prompt scores wrong the only useful question is which four moves
    // the player left, and that is here rather than reachable only by replay.
    // Same idiom as `longplay`'s `steps.ndjson`.
    {
        let mut out = String::new();
        for a in &attempts {
            out.push_str(&format!(
                "{{\"seed\":{},\"goal\":{},\"ending\":{:?},\"frames\":{},\
                 \"decisions\":{},\"battles\":{},\"won\":{},\"lost\":{},\
                 \"fled\":{},\"moves\":[{},{},{},{}],\"correct\":{},\
                 \"decline\":{}}}\n",
                a.seed,
                a.goal,
                a.ending,
                a.frames,
                a.decisions,
                a.battles,
                a.won,
                a.lost,
                a.fled,
                a.moves[0],
                a.moves[1],
                a.moves[2],
                a.moves[3],
                match a.correct {
                    Some(v) => v.to_string(),
                    None => "null".into(),
                },
                a.decline
            ));
        }
        let _ = std::fs::write(dir.join("attempts.ndjson"), out);
    }
    use std::sync::atomic::Ordering::Relaxed;
    let timing = abrig::Timing {
        attempts: attempts.len() as u32,
        wall_ms: tot[0].load(Relaxed),
        emulate_ms: tot[1].load(Relaxed),
        setup_ms: tot[2].load(Relaxed),
        frames: tot[3].load(Relaxed),
    };
    abrig::Cell { attempts, timing }
}

fn banner(p: &abrig::Plan, out: &Path) -> String {
    let mut s = String::new();
    s.push_str(&format!("\n{}\n", "=".repeat(78)));
    s.push_str(&format!("TEST RIG — {}\n", p.name));
    if !p.question.is_empty() {
        s.push_str(&format!("{}\n", p.question));
    }
    s.push_str(&format!(
        "SPLIT: {} — {}\n",
        p.split.word(),
        p.split.meaning()
    ));
    s.push_str(&format!("{}\n", "=".repeat(78)));
    s.push_str(&format!(
        "scenario {}\nentries  {}\nattempts {} per arm per subject, seed {}.., phase spread {}\n\
         output   {}\n",
        p.scenario.display(),
        p.pool.display(),
        if p.runs_is_pool {
            "one per entry state".to_string()
        } else {
            p.runs.to_string()
        },
        p.seed0,
        p.idle_spread,
        out.display()
    ));
    s
}

/// The whole report, as the text file and as what is printed.
fn render(
    plan: &abrig::Plan,
    subjects: &[abrig::Subject],
    cells: &HashMap<(String, String), abrig::Cell>,
    pools: &HashMap<String, usize>,
) -> String {
    let mut s = String::new();
    let reference = plan
        .arms
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let get = |sub: &str, arm: &str| cells.get(&(sub.to_string(), arm.to_string()));

    for sub in subjects {
        let Some(&pool) = pools.get(&sub.name) else {
            continue;
        };
        s.push_str(&format!(
            "\n── {} ─────────────────────────────────────────────\n\
             {pool} entry state(s) in the pool\n\n",
            sub.name
        ));
        s.push_str(
            "  arm          goal              battles  won    fled   lost   \
             HP left  decisions\n",
        );
        for arm in &plan.arms {
            let Some(c) = get(&sub.name, &arm.name) else {
                continue;
            };
            let b = c.battles().max(1) as f64;
            s.push_str(&format!(
                "  {:<12} {:<17} {:>7}  {:>5.1}% {:>5.1}% {:>5.1}% {:>7.1}% {:>9.1}\n",
                arm.name,
                abrig::pct_ci(c.goals(), c.n()),
                c.battles(),
                100.0 * c.won() as f64 / b,
                100.0 * c.fled() as f64 / b,
                100.0 * c.lost() as f64 / b,
                c.hp_left(),
                c.mean_decisions(),
            ));
        }
        // How each arm failed, which a rate cannot say.
        for arm in &plan.arms {
            let Some(c) = get(&sub.name, &arm.name) else {
                continue;
            };
            let e = c.endings();
            if e.len() > 1 || e.first().map(|x| x.0.as_str()) != Some("goal") {
                let list: Vec<String> =
                    e.iter().map(|(k, v)| format!("{k} {v}")).collect();
                s.push_str(&format!(
                    "  {:<12} endings: {}\n",
                    arm.name,
                    list.join(", ")
                ));
            }
        }
        // The move-learning prompts, when the pool carried expected answers.
        // **The unit of a move-learning test is a prompt**, not an episode: what
        // was held, what was offered, what each player did, and whether they
        // agree — see `docs/agent/test-rig.md` § "Measuring move learning".
        if plan
            .arms
            .iter()
            .filter_map(|arm| get(&sub.name, &arm.name))
            .any(|c| c.prompts().1 > 0)
        {
            // **Declines on their own column.** A decline's correct answer is
            // the move set the prompt arrived with, so a player that answers
            // nothing scores every one of them — which is what the `mash-a`
            // floor did, and pooling the two hid it.
            s.push_str(
                "\n  arm          prompts answered correctly              of which\n",
            );
            for arm in &plan.arms {
                let Some(c) = get(&sub.name, &arm.name) else {
                    continue;
                };
                let (k, n) = c.prompts();
                let ((rk, rn), (dk, dn)) = c.prompts_split();
                s.push_str(&format!(
                    "  {:<12} {:>4} of {:<4} {}   replace {:>2}/{:<2} decline {:>2}/{:<2}\n",
                    arm.name,
                    k,
                    n,
                    abrig::pct_ci(k, n),
                    rk,
                    rn,
                    dk,
                    dn
                ));
            }
            for arm in plan.arms.iter().skip(1) {
                let (Some(r), Some(c)) =
                    (get(&sub.name, &reference), get(&sub.name, &arm.name))
                else {
                    continue;
                };
                let (same, pairs) = r.agreement(c);
                if pairs > 0 {
                    s.push_str(&format!(
                        "  AGREEMENT {reference} vs {}: {} of {pairs} prompts left the \
                         same four moves ({})\n",
                        arm.name,
                        same,
                        abrig::pct_ci(same, pairs)
                    ));
                }
            }
        }
        // The battle underneath the pass rate, per arm, with its own interval.
        // On the very first subject this rig was pointed at, this column moved
        // by fifty points while the goal column did not move at all.
        s.push_str("\n  arm          battles won (of the attempts that fought)\n");
        for arm in &plan.arms {
            let Some(c) = get(&sub.name, &arm.name) else {
                continue;
            };
            let (k, n) = c.tally(abrig::Outcome::BattleWon);
            s.push_str(&format!(
                "  {:<12} {:>4} of {:<4} {}\n",
                arm.name,
                k,
                n,
                abrig::pct_ci(k, n)
            ));
        }
        // The gaps, against the first arm, on both outcomes.
        s.push('\n');
        for arm in plan.arms.iter().skip(1) {
            let (Some(r), Some(c)) =
                (get(&sub.name, &reference), get(&sub.name, &arm.name))
            else {
                continue;
            };
            for o in [
                abrig::Outcome::Goal,
                abrig::Outcome::PromptCorrect,
                abrig::Outcome::BattleWon,
            ] {
                match abrig::gap_on(r, c, o) {
                    Some(g) => {
                        let (lo, hi) = g.interval();
                        s.push_str(&format!(
                            "  GAP [{}] {reference} \u{2212} {}: {:+.1} points \
                             [{:+.1},{:+.1}] over {} pair(s)  ({} only {reference}, \
                             {} only {}; McNemar p={:.4})\n",
                            o.label(),
                            arm.name,
                            g.points,
                            lo,
                            hi,
                            g.pairs,
                            g.only_reference,
                            g.only_arm,
                            arm.name,
                            g.p
                        ));
                    }
                    None if o == abrig::Outcome::Goal => s.push_str(&format!(
                        "  GAP [goal] {reference} \u{2212} {}: refused \u{2014} the two arms \
                         did not play the same seeds, so there is no pairing to test\n",
                        arm.name
                    )),
                    None => s.push_str(&format!(
                        "  GAP [{}] {reference} \u{2212} {}: no pairs \u{2014} {}\n",
                        o.label(),
                        arm.name,
                        match o {
                            abrig::Outcome::BattleWon =>
                                "the two arms never both fought on the same seed",
                            abrig::Outcome::PromptCorrect =>
                                "no entry state carried an expected answer, so there                                  were no prompts to score",
                            abrig::Outcome::Goal => "there is no pairing",
                        }
                    )),
                }
            }
        }
    }

    // Is the gap the same on every subject? Never an average — see the module
    // docs of `examples/support/abrig.rs`.
    if subjects.len() > 1 && plan.arms.len() > 1 {
        s.push_str("\nIS THE GAP THE SAME ON EVERY SUBJECT?\n");
        for arm in plan.arms.iter().skip(1) {
            for o in [
                abrig::Outcome::Goal,
                abrig::Outcome::PromptCorrect,
                abrig::Outcome::BattleWon,
            ] {
                let mut gs: Vec<(String, abrig::Gap)> = Vec::new();
                for sub in subjects {
                    if let (Some(r), Some(c)) =
                        (get(&sub.name, &reference), get(&sub.name, &arm.name))
                    {
                        if let Some(g) = abrig::gap_on(r, c, o) {
                            gs.push((sub.name.clone(), g));
                        }
                    }
                }
                for pair in gs.windows(2) {
                    let (d, se, p) = abrig::gaps_differ(&pair[0].1, &pair[1].1);
                    let verdict = if p < 0.05 {
                        "THE GAP ITSELF DIFFERS \u{2014} something subject-specific is \
                         happening in this arm"
                    } else {
                        "cannot be separated at this sample size \u{2014} consistent with \
                         one gap and two different ceilings"
                    };
                    s.push_str(&format!(
                        "  [{}] {} : {} vs {} \u{2014} gaps {:+.1} and {:+.1}, difference \
                         {:+.1} (se {:.1}, p={:.3})\n    {verdict}\n",
                        o.label(),
                        arm.name,
                        pair[0].0,
                        pair[1].0,
                        pair[0].1.points,
                        pair[1].1.points,
                        d,
                        se,
                        p
                    ));
                }
            }
        }
    }

    s.push_str(&format!(
        "\nWHAT THIS CANNOT TELL YOU\n  \
         The split is {}. {}\n  \
         The gap is paired on the seed, so it is a within-fight difference and \
         says nothing\n  about a different link, a different opponent or a \
         different cartridge.\n  \
         The battle columns are what the host could see: outcomes, not \
         reasoning. Whether a\n  move was well chosen is not in them.\n",
        plan.split.word(),
        plan.split.meaning()
    ));
    s
}

/// Where the wall clock went, per arm.
///
/// **A rig like this is worth having only if it is fast**, so it says which
/// part to attack rather than leaving anyone to guess. The three buckets add
/// up to the attempts' own wall clock; the run's total is larger by whatever
/// the workers were not all busy for.
fn breakdown(
    plan: &abrig::Plan,
    subjects: &[abrig::Subject],
    cells: &HashMap<(String, String), abrig::Cell>,
    workers: u32,
    run_seconds: f64,
) -> String {
    let mut s = String::from("\nWHERE THE WALL CLOCK WENT\n");
    s.push_str(
        "  arm          attempts    frames   worker-s  emulating   setup            deciding/loop\n",
    );
    let mut grand = abrig::Timing::default();
    for arm in &plan.arms {
        let mut t = abrig::Timing::default();
        for sub in subjects {
            if let Some(c) = cells.get(&(sub.name.clone(), arm.name.clone())) {
                t.attempts += c.timing.attempts;
                t.wall_ms += c.timing.wall_ms;
                t.emulate_ms += c.timing.emulate_ms;
                t.setup_ms += c.timing.setup_ms;
                t.frames += c.timing.frames;
            }
        }
        if t.attempts == 0 {
            continue;
        }
        s.push_str(&format!(
            "  {:<12} {:>8} {:>9} {:>9.1} {:>7.1}% {:>7.1}% {:>13.1}%\n",
            arm.name,
            t.attempts,
            t.frames,
            t.wall_ms as f64 / 1000.0,
            t.share(t.emulate_ms),
            t.share(t.setup_ms),
            t.share(t.other_ms()),
        ));
        grand.attempts += t.attempts;
        grand.wall_ms += t.wall_ms;
        grand.emulate_ms += t.emulate_ms;
        grand.setup_ms += t.setup_ms;
        grand.frames += t.frames;
    }
    s.push_str(&format!(
        "  {:<12} {:>8} {:>9} {:>9.1} {:>7.1}% {:>7.1}% {:>13.1}%\n",
        "ALL",
        grand.attempts,
        grand.frames,
        grand.wall_ms as f64 / 1000.0,
        grand.share(grand.emulate_ms),
        grand.share(grand.setup_ms),
        grand.share(grand.other_ms()),
    ));
    let busy = grand.wall_ms as f64 / 1000.0;
    s.push_str(&format!(
        "  {} of wall clock on {workers} worker(s) = {:.1} worker-seconds, of which \
         {:.1} were inside an attempt ({:.0}% busy)\n  \
         {:.0} emulated frames/s aggregate; setup is {:.0} ms per attempt\n  \
         `deciding/loop` is the run loop, the referee and the plugin. **It is \
         NOT the player's think time**: the emulator never\n  blocks on a \
         driver, so a subprocess arm's spawn-and-decide happens while the \
         machine keeps running and is\n  charged to `emulating`. A slow player \
         shows up as more frames per attempt, not as a bigger share here.\n",
        abrig::hms(run_seconds),
        run_seconds * workers as f64,
        busy,
        100.0 * busy / (run_seconds * workers as f64).max(0.001),
        grand.frames as f64 / run_seconds.max(0.001),
        grand.setup_ms as f64 / grand.attempts.max(1) as f64,
    ));
    s
}

fn report_json(
    plan: &abrig::Plan,
    subjects: &[abrig::Subject],
    cells: &HashMap<(String, String), abrig::Cell>,
    pools: &HashMap<String, usize>,
) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!(
        "  \"schema\": \"gbtestrig-report/1\",\n  \"name\": {:?},\n  \
         \"question\": {:?},\n  \"split\": {:?},\n  \"scenario\": {:?},\n  \
         \"entry_pool\": {:?},\n  \"runs\": {},\n  \"seed0\": {},\n  \
         \"idle_spread\": {},\n  \"subjects\": [\n",
        plan.name,
        plan.question,
        plan.split.word(),
        plan.scenario.display().to_string(),
        plan.pool.display().to_string(),
        plan.runs,
        plan.seed0,
        plan.idle_spread
    ));
    let reference = plan
        .arms
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let mut first_sub = true;
    for sub in subjects {
        let Some(&pool) = pools.get(&sub.name) else {
            continue;
        };
        if !first_sub {
            out.push_str(",\n");
        }
        first_sub = false;
        out.push_str(&format!(
            "    {{\"name\": {:?}, \"lead_species\": {}, \"pool\": {pool}, \"arms\": [\n",
            sub.name,
            sub.lead_json()
        ));
        let mut first_arm = true;
        for arm in &plan.arms {
            let Some(c) = cells.get(&(sub.name.clone(), arm.name.clone())) else {
                continue;
            };
            if !first_arm {
                out.push_str(",\n");
            }
            first_arm = false;
            let (lo, hi) = abrig::wilson(c.goals(), c.n());
            let one = |o: abrig::Outcome| -> String {
                if arm.name == reference {
                    return "null".into();
                }
                match cells
                    .get(&(sub.name.clone(), reference.clone()))
                    .and_then(|r| abrig::gap_on(r, c, o))
                {
                    Some(g) => {
                        let (glo, ghi) = g.interval();
                        format!(
                            "{{\"points\": {:.3}, \"lo\": {:.3}, \"hi\": {:.3}, \
                             \"pairs\": {}, \"only_reference\": {}, \"only_arm\": {}, \
                             \"p\": {:.6}}}",
                            g.points,
                            glo,
                            ghi,
                            g.pairs,
                            g.only_reference,
                            g.only_arm,
                            g.p
                        )
                    }
                    None => "null".into(),
                }
            };
            let g = one(abrig::Outcome::Goal);
            let gb = one(abrig::Outcome::BattleWon);
            let gp = one(abrig::Outcome::PromptCorrect);
            let (pk, pn) = c.prompts();
            let (agree, apairs) = cells
                .get(&(sub.name.clone(), reference.clone()))
                .map(|r| r.agreement(c))
                .unwrap_or((0, 0));
            let (bk, bn) = c.tally(abrig::Outcome::BattleWon);
            let (blo, bhi) = abrig::wilson(bk, bn);
            out.push_str(&format!(
                "      {{\"name\": {:?}, \"player\": {:?}, \"n\": {}, \"goals\": {}, \
                 \"rate\": {:.5}, \"ci_lo\": {:.5}, \"ci_hi\": {:.5}, \
                 \"battles\": {}, \"won\": {}, \"fled\": {}, \"lost\": {}, \
                 \"hp_left_percent\": {:.2}, \"mean_decisions\": {:.2}, \
                 \"mean_frames\": {:.1}, \"battle_won\": {bk}, \
                 \"battle_fought\": {bn}, \"battle_ci_lo\": {blo:.5}, \
                 \"battle_ci_hi\": {bhi:.5}, \"prompts\": {pn}, \
                 \"prompts_correct\": {pk}, \"agreed_with_reference\": {agree}, \
                 \"agreement_pairs\": {apairs}, \"wall_ms\": {}, \
                 \"emulate_ms\": {}, \"setup_ms\": {}, \"gap\": {g}, \
                 \"gap_prompt\": {gp}, \"gap_battle\": {gb}}}",
                arm.name,
                arm.player.kind(),
                c.n(),
                c.goals(),
                c.rate(),
                lo,
                hi,
                c.battles(),
                c.won(),
                c.fled(),
                c.lost(),
                c.hp_left(),
                c.mean_decisions(),
                c.mean_frames(),
                c.timing.wall_ms,
                c.timing.emulate_ms,
                c.timing.setup_ms
            ));
        }
        out.push_str("\n    ]}");
    }
    out.push_str("\n  ]\n}\n");
    out
}

// ---------------------------------------------------------------------------
// The side-by-side cut
// ---------------------------------------------------------------------------

/// The Game Boy's own picture, in pixels.
const GB_W: usize = 160;
const GB_H: usize = 144;
/// How much to magnify each arm's picture. Integer, nearest-neighbour, so no
/// colour appears that was not in the framebuffer — the categorical guard
/// `screen::halfblocks` and `examples/swarm_view.rs` both apply.
const AB_ZOOM: usize = 3;
/// Rows above each picture for its label, and below for the caption.
const AB_HEAD: usize = 36;
const AB_FOOT: usize = 22;
const AB_PAD: usize = 8;

/// **The picture beside the number.** Two or more arms playing the *same* fight
/// from the *same* entry state at the *same* seed, laid out left to right, one
/// PNG per sampled frame.
///
/// A table says a player is forty points behind; it cannot say whether it is
/// battling badly or standing in a doorway pressing A, and those want completely
/// different work. Judging that is what a person does in ten seconds of footage
/// and cannot do from any summary.
///
/// **The swarm compositor is deliberately not reused.** `examples/swarm_view.rs`
/// draws agents as sprites on the world atlas from their position tracks, which
/// is exactly right for six hundred agents crossing Kanto and exactly wrong for
/// a battle: a battle has no map position at all, so every agent would be a
/// motionless dot. What a fight needs is the framebuffer itself.
fn cmd_abfilm(a: &Args) {
    let rom = load_cart(&need_rom(a));
    let plan = build_plan(a, &rom);
    let out = a.out.clone().unwrap_or_else(|| {
        PathBuf::from("target/testrig")
            .join(&plan.name)
            .join("film")
    });
    std::fs::create_dir_all(&out).expect("output directory");
    // How many output frames to aim for, not a stride: the arms differ in
    // length by an order of magnitude on this link, so a fixed stride gives a
    // five-minute film of one of them flailing. `swarm_film.py` reached the
    // same rule from the other end — sample from the emulated time to cover,
    // and print the ratio on screen.
    let frames_out = if a.runs > 1 { a.runs as usize } else { 600 };

    // The pool, filtered to the subject the caller named — or the whole pool.
    let want = a.leads.first().and_then(|s| lead_arg(&rom, s));
    let mut files: Vec<PathBuf> = std::fs::read_dir(&plan.pool)
        .unwrap_or_else(|e| {
            eprintln!("cannot read the entry pool {}: {e}", plan.pool.display());
            exit(1);
        })
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "gbss").unwrap_or(false))
        .collect();
    files.sort();
    let pool: Vec<Vec<u8>> = files
        .iter()
        .map(|p| std::fs::read(p).expect("entry state"))
        .filter(|b| want.map(|w| lead_species(&rom, b) == w).unwrap_or(true))
        .collect();
    if pool.is_empty() {
        eprintln!("no entry state in the pool matches");
        exit(1);
    }
    let seed = a.seed;
    let entry = pool[(seed as usize) % pool.len()].clone();
    let idle = phase_for(seed, plan.idle_spread);
    let subject = a
        .leads
        .first()
        .cloned()
        .unwrap_or_else(|| "any starter".into());

    // One arm at a time, each writing its own raw stream. Sequential on
    // purpose: this is one attempt each, and the frames must line up.
    let mut streams: Vec<(String, Vec<u8>, String)> = Vec::new();
    for arm in &plan.arms {
        let dir = out.join(&arm.name);
        std::fs::create_dir_all(&dir).ok();
        let raw = dir.join("frames.rgba");
        let _ = std::fs::remove_file(&raw);
        let mut spec = terminalgb::video::VideoSpec::new(
            terminalgb::video::Destination::Path(raw.clone()),
        );
        // Emulated pacing, not wall clock: a cut of a scripted run must be a
        // function of the run and not of how busy the box was.
        spec.pace = terminalgb::video::Pace::Emulated;
        let video = {
            let (v, warn) = terminalgb::video::VideoOut::open(spec);
            if let Some(w) = warn {
                eprintln!("{}: {w}", arm.name);
            }
            v
        };
        let mk: DriverFactory = match &arm.player {
            abrig::Player::Expert => Box::new(move |name: &str| {
                Box::new(ChainDriver::new(name, seed)) as Box<dyn Driver>
            }),
            other => {
                let other = other.clone();
                Box::new(move |_: &str| make_driver(&other, seed))
            }
        };
        let frames_dir =
            matches!(arm.player, abrig::Player::Command { .. }).then(|| dir.join("snap"));
        let o = play_filmed(
            &rom,
            &plan.scenario,
            Some(entry.clone()),
            &dir,
            idle,
            mk.as_ref(),
            frames_dir.as_deref(),
            video,
        );
        let bytes = std::fs::read(&raw).unwrap_or_default();
        let caption = format!(
            "{} - {} frames - {} decisions",
            o.ending, o.frames, o.decisions
        );
        println!(
            "  {:<12} {:>7} frames  {}",
            arm.name,
            bytes.len() / (GB_W * GB_H * 4),
            caption
        );
        streams.push((arm.name.clone(), bytes, caption));
    }

    compose_side_by_side(&out, &streams, &plan.name, &subject, seed, frames_out);
}

/// [`play_with`] with a video sink attached and the picture left on.
#[allow(clippy::too_many_arguments)]
fn play_filmed(
    rom: &Cart,
    scenario_path: &Path,
    start: Option<Vec<u8>>,
    out: &Path,
    idle: u32,
    driver: &dyn Fn(&str) -> Box<dyn Driver>,
    frames_dir: Option<&Path>,
    video: terminalgb::video::VideoOut,
) -> Outcome {
    let s = Scenario::load(scenario_path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", scenario_path.display());
        exit(1);
    });
    std::fs::create_dir_all(out).ok();
    let mut gb = Gameboy::new(rom.0.as_ref().clone(), None);
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    gb.set_render_pixels(true);
    gb.set_render_audio(false);
    let start = phase_start(&mut gb, start, idle);
    let cfg = Config {
        budget: Budget {
            max_calls: s.episode().max_calls,
            ..Budget::default()
        },
        objective: s.objective.clone(),
        frame_capture: frames_dir.map(|d| d.to_path_buf()),
        ..Config::default()
    };
    gb.add_plugin(Box::new(AgentPlugin::new(driver(&s.name), cfg)))
        .expect("attach the agent");
    let run = ScenarioRun::new(gb, s.clone(), scenario_path, out.to_path_buf(), start)
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            exit(1);
        })
        .quiet()
        .without_captures()
        .filming(video);
    let r = run.play();
    Outcome {
        goal: r.reached_goal(),
        frames: r.recording.frames,
        decisions: r.recording.decisions.len(),
        ending: r.recording.ending.tag().to_string(),
        detail: String::new(),
        map: r.final_progress.map,
        x: r.final_progress.x,
        y: r.final_progress.y,
        wall_ms: r.wall_ms,
        emulate_ms: r.emulate_ms,
        setup_ms: 0,
        frames_total: r.recording.frames + idle as u64,
        state: r.final_state_bytes.clone(),
        battles: r.battles,
        recording: r.recording,
        path: r.path,
    }
}

/// The longest complete form of a caption that fits, never a truncation.
fn fit_caption(full: &str, width: u32, scale: u32) -> String {
    use terminalgb::video::card::text_width;
    if text_width(full, scale) <= width {
        return full.to_string();
    }
    // Drop trailing clauses one at a time. Every intermediate form is a whole
    // sentence, so a reader is never shown half a number.
    let parts: Vec<&str> = full.split(" - ").collect();
    for keep in (1..parts.len()).rev() {
        let short = parts[..keep].join(" - ");
        if text_width(&short, scale) <= width {
            return short;
        }
    }
    // Even the first clause is too wide: drop a scale instead of cutting it.
    parts[0].to_string()
}

/// Lay the arms out left to right, one PNG per sampled frame.
///
/// **A short arm is held on its last frame rather than going blank.** They do
/// not finish together — one may reach the goal in a third of the other's
/// decisions — and a panel that empties reads as a crash rather than as
/// "finished earlier", which is the opposite of what happened.
fn compose_side_by_side(
    out: &Path,
    streams: &[(String, Vec<u8>, String)],
    plan: &str,
    subject: &str,
    seed: u64,
    frames_out: usize,
) {
    use terminalgb::video::card::Canvas;
    let frame_bytes = GB_W * GB_H * 4;
    let counts: Vec<usize> = streams
        .iter()
        .map(|(_, b, _)| b.len() / frame_bytes)
        .collect();
    let longest = counts.iter().copied().max().unwrap_or(0);
    if longest == 0 {
        eprintln!("no frames were captured — was the picture on?");
        exit(1);
    }
    let pane_w = GB_W * AB_ZOOM;
    let pane_h = GB_H * AB_ZOOM;
    let w = streams.len() * pane_w + (streams.len() + 1) * AB_PAD;
    let h = AB_HEAD + pane_h + AB_FOOT;
    let bg = [16u8, 18, 22, 255];
    let ink = [232u8, 236, 240, 255];
    let dim = [140u8, 148, 158, 255];

    // **Round the stride UP, never down.** Flooring gives a film that stops
    // before the longest arm finishes, with the ending — the part worth
    // watching — missing and nothing saying so.
    let every = longest.div_ceil(frames_out.max(1)).max(1);
    let dir = out.join("frames");
    std::fs::create_dir_all(&dir).expect("frames directory");
    let mut written = 0usize;
    let mut i = 0usize;
    while i < longest {
        let mut buf = vec![0u8; w * h * 4];
        {
            let mut c = Canvas::new(&mut buf, w as u32, h as u32);
            c.fill(0, 0, w as u32, h as u32, bg);
            c.text(
                AB_PAD as i64,
                3,
                &format!("{plan} - {subject} - seed {seed}"),
                1,
                dim,
            );
        }
        for (k, (name, bytes, caption)) in streams.iter().enumerate() {
            // Hold the last frame rather than blanking a finished arm.
            let idx = i.min(counts[k].saturating_sub(1));
            let src = &bytes[idx * frame_bytes..(idx + 1) * frame_bytes];
            let x0 = AB_PAD + k * (pane_w + AB_PAD);
            for y in 0..pane_h {
                let sy = y / AB_ZOOM;
                for x in 0..pane_w {
                    let sx = x / AB_ZOOM;
                    let s = (sy * GB_W + sx) * 4;
                    let d = ((AB_HEAD + y) * w + x0 + x) * 4;
                    buf[d..d + 4].copy_from_slice(&src[s..s + 4]);
                }
            }
            let mut c = Canvas::new(&mut buf, w as u32, h as u32);
            c.text(x0 as i64, 13, &name.to_uppercase(), 3, ink);
            let tail = if idx + 1 >= counts[k] && counts[k] > 0 {
                caption.clone()
            } else {
                format!("frame {}", idx)
            };
            // **Shorten through complete forms; never clip.** A truncated
            // caption is worse than a short one, because a reader cannot tell
            // it was cut — the same rule `fit_bar` follows in the frontend.
            c.text(
                x0 as i64,
                (AB_HEAD + pane_h + 5) as i64,
                &fit_caption(&tail, pane_w as u32, 2),
                2,
                dim,
            );
        }
        write_rgba(&dir.join(format!("{written:06}.png")), w, h, &buf);
        written += 1;
        i += every;
    }
    println!(
        "\n{written} frame(s) at {w}x{h}, one per {every} emulated frames \
         ({:.1}x timelapse at 30 fps)\n{}\n\n\
         ffmpeg -y -framerate 30 -i {}/%06d.png -c:v libopenh264 -b:v 4M \\\n  \
         -pix_fmt yuv420p {}\n\
         (this box's ffmpeg is built without libx264 — libopenh264 is the \
          substitute, and -crf is not accepted there)",
        every as f64 * 30.0 / 59.7275,
        dir.display(),
        dir.display(),
        out.join("cut.mp4").display()
    );
}
