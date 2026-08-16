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
        "05-the-rival-battle" => {
            let x = 4 + (seed % 2) as u8;
            let approach = [(2u8, 4u8), (8, 4), (2, 5), (8, 5)][(seed % 4) as usize];
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
            r.insert(
                VIRIDIAN_CITY,
                vec![stage(
                    &[(29, 26), (19, 26), (19, 30), (20, 30), (20, 35)],
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
    matches!(
        scenario,
        "05-the-rival-battle" | "10-back-to-viridian" | "11-train-for-the-forest"
    )
}

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
            primitive: false,
            trail: None,
            text_tries: 0,
            last_text: String::new(),
            name_row0: None,
            name_want: 0,
            fight_wild: may_fight_wild(scenario),
            heal_route: heal_route(scenario),
            healing: false,
            train_to: 13,
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
        //    policy has a band where it does neither. Measured with healing at
        //    a third and fighting at a half: the lead settled at 8 of 23 HP —
        //    too hurt to pick a fight, not hurt enough to go and get healed —
        //    and ran from everything for 2,402 decisions.
        if !self.heal_route.is_empty() && s.lead_max_hp > 0 {
            let hurt = s.lead_hp * 2 <= s.lead_max_hp;
            let full = s.lead_hp >= s.lead_max_hp;
            if !self.healing && hurt {
                self.healing = true;
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

    /// Battle policy. A trainer battle cannot be run from, so it is fought; a
    /// wild one is run from, which is also what the `battle flee` rule on the
    /// route legs requires.
    fn fight(&mut self, s: &Snap) -> String {
        self.battle_turns += 1;
        let sel = s.selected.to_ascii_uppercase();
        // A submenu nothing here wants. B is the one button that can only ever
        // back out of one.
        if s.said("CHOOSE A POK") {
            return self.one("Back out of the party list.", "b");
        }
        if s.waiting || s.kind == "dialogue" || s.kind == "text" {
            if self.primitive {
                return self.one("Let the battle text finish.", "a");
            }
            return Plan::new("Let the battle text finish.").text(20).finish();
        }
        let wild = s.battle.as_deref() == Some("wild");
        // **Fighting is a rule, not a preference.** A `battle flee` leg
        // forfeits the moment a battle is *won*, so on those legs the answer
        // is always RUN whatever the party looks like. Where fighting is
        // allowed, a wild Pokémon is worth beating only while the lead is
        // under-levelled and healthy enough to take the hits.
        let train = self.fight_wild
            && s.lead_level < self.train_to
            && s.lead_hp * 2 > s.lead_max_hp;
        let running = wild && !train;

        // **Navigate the battle menu one key at a time, off the cursor.**
        // The menu is FIGHT / PKMN on the top row and ITEM / RUN on the
        // bottom, so "down then right" reaches RUN from FIGHT — except that
        // Gen 1's `HandleMenuInput` delays several frames after accepting a
        // press, so the *first* of a burst is routinely swallowed and the
        // rest land one step out of phase. Measured: a run policy that fired
        // down/right/A blind opened the party list instead, over and over,
        // for 2,500 decisions. Reading `▶` and pressing one key cannot go out
        // of phase, because a swallowed press simply repeats.
        if s.kind == "menu" {
            let on_main = s.said("FIGHT");
            if !on_main {
                // The move list. Its first slot is the damaging one on every
                // Gen 1 starter at these levels.
                return self.one("Use the first move.", "a");
            }
            if running {
                return if sel.contains("RUN") {
                    self.one("Run.", "a")
                } else if sel.contains("FIGHT") {
                    self.one("Down towards RUN.", "down")
                } else if sel.contains("ITEM") {
                    self.one("Right to RUN.", "right")
                } else {
                    self.one("Down towards RUN.", "down")
                };
            }
            return if sel.contains("FIGHT") {
                self.one("Fight.", "a")
            } else if sel.contains("ITEM") || sel.contains("RUN") {
                self.one("Up to FIGHT.", "up")
            } else {
                self.one("Left to FIGHT.", "left")
            };
        }
        if self.primitive {
            return self.one("Let the battle run on.", "a");
        }
        Plan::new("Let the battle run on.").text(20).finish()
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
            eprintln!(
                "  [{:3}] map 0x{:02X} ({:2},{:2}) stage {} idx {} kind {:8} lv{} \
                 hp{}/{} :: [{text}] {short}",
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

  film --rom R --agents N [--out DIR] [--workers N]
      Play the WHOLE chain once per agent, each agent carrying its own exit
      state forward, and write one continuous per-frame position track each.
      An agent that fails a link stops there, which is the honest picture.

  sprites --rom R --start FILE --out DIR
      Read the player's own overworld sprite out of OAM and VRAM, four
      facings, as transparent PNGs. Game art: write it outside the repository.

  verify --rom R --scenario S [--start FILE] [--runs N] [--primitive]
      Play the same seeds serially and in parallel and require identical
      final states. Determinism outranks everything else in flight.
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
    primitive: bool,
    paths: bool,
    start_dir: Option<PathBuf>,
    /// Frames of phase to spread attempts over; 0 disables the axis.
    idle_spread: u32,
    /// How many of a corpus's successful exits to keep as `.gbss`, for the
    /// next link to use as its entry pool.
    exits: u32,
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
            "--out" => a.out = Some(PathBuf::from(next(i))),
            "--seed" => a.seed = next(i).parse().unwrap_or(1),
            "--seed0" => a.seed0 = next(i).parse().unwrap_or(0),
            "--runs" => a.runs = next(i).parse().unwrap_or(1).max(1),
            "--seeds" => a.seeds = next(i).parse().unwrap_or(1).max(1),
            "--from" => a.from = next(i).parse().unwrap_or(1),
            "--to" => a.to = next(i).parse().unwrap_or(12),
            "--workers" => a.workers = next(i).parse().unwrap_or(1).max(1),
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
fn stage_rom(rom: &Path, out: &Path) -> PathBuf {
    std::fs::create_dir_all(out).expect("create output directory");
    let dest = out.join("cart.gb");
    let bytes = std::fs::read(rom).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", rom.display());
        exit(1);
    });
    std::fs::write(&dest, &bytes).expect("stage the cartridge");
    dest
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
    state: Vec<u8>,
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
    rom: &Path,
    scenario_path: &Path,
    start: Option<Vec<u8>>,
    seed: u64,
    out: &Path,
    quiet: bool,
    primitive: bool,
    track: bool,
    idle: u32,
) -> Outcome {
    let s = Scenario::load(scenario_path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", scenario_path.display());
        exit(1);
    });
    let staged = stage_rom(rom, out);
    let data = std::fs::read(&staged).expect("read staged cartridge");
    let mut gb = Gameboy::new(data, Some(staged));
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
    // **Phase the world before the agent is attached, never after.** Burning
    // the frames here means the plugin does not exist yet and so cannot act
    // during them; the machine is simply left `idle` frames further on, and
    // that state becomes the attempt's start. See [`phase_start`].
    let start = phase_start(&mut gb, start, idle);
    let cfg = Config {
        budget: Budget {
            max_calls: s.episode().max_calls,
            ..Budget::default()
        },
        objective: s.objective.clone(),
        ..Config::default()
    };
    gb.add_plugin(Box::new(AgentPlugin::new(
        Box::new(ChainDriver::new(&s.name, seed).primitive(primitive)),
        cfg,
    )))
    .expect("attach the agent");

    let mut run =
        ScenarioRun::new(gb, s.clone(), scenario_path, out.to_path_buf(), start)
            .unwrap_or_else(|e| {
                eprintln!("{e}");
                exit(1);
            })
            .tracking(track);
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
        wall_ms: r.wall_ms,
        state: r.final_state_bytes.clone(),
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
        "solve" => cmd_solve(&a),
        "chain" => cmd_chain(&a),
        "corpus" => cmd_corpus(&a),
        "verify" => cmd_verify(&a),
        "sprites" => cmd_sprites(&a),
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
    let rom = need_rom(a);
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
    let rom = need_rom(a);
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
    let rom = need_rom(a);
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
    let pool = entry_pool(a);
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
    let rom = need_rom(a);
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
    let rom = need_rom(a);
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/sprites"));
    std::fs::create_dir_all(&out).expect("output directory");
    let staged = stage_rom(&rom, &out);
    let data = std::fs::read(&staged).expect("read staged cartridge");
    let mut gb = Gameboy::new(data, Some(staged));
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
fn cmd_film(a: &Args) {
    let rom = need_rom(a);
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/pewter-chain/film"));
    std::fs::create_dir_all(&out).expect("output directory");
    let files = chain_files();
    let agents = a.runs.max(1);
    let workers = a.workers.max(1);
    println!(
        "{agents} agent(s) over {} link(s), {workers} worker(s)",
        files.len()
    );

    let t = std::time::Instant::now();
    let next = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let done =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, usize, u64)>::new()));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            let (next, done) = (next.clone(), done.clone());
            let (rom, out, files) = (rom.clone(), out.clone(), files.clone());
            let primitive = a.primitive;
            let spread = a.idle_spread;
            sc.spawn(move || loop {
                let ag = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if ag >= agents {
                    break;
                }
                let seed = 1 + ag as u64;
                let dir = out.join(format!("w{ag}"));
                let mut carried: Option<Vec<u8>> = None;
                let mut track: Vec<terminalgb::scenario::run::PathPoint> = Vec::new();
                let mut base = 0u32;
                let mut reached = 0usize;
                for (n, f) in files.iter().enumerate() {
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
                    base = track.last().map(|p| p.frame).unwrap_or(base);
                    if !o.goal {
                        break;
                    }
                    reached = n + 1;
                    carried = Some(o.state);
                }
                write_path(&out.join(format!("agent-{ag:02}.csv")), &track);
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
