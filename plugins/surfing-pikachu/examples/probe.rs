//! Play a run headlessly under a fixed policy and report what happened.
//!
//! `probe <seed> <policy>` where policy is `ride` (touch nothing), `oneflip`
//! (one swipe per jump, through the gesture layer) or `spin` (hold a
//! direction the whole time you are airborne).
use surfing_pikachu::game::{Game, PikachuState, Status};
use surfing_pikachu::input::{Controls, Dir, Gesture};

fn main() {
    let seed: u32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let policy = std::env::args().nth(2).unwrap_or_else(|| "ride".into());
    let mut g = Game::new(seed);
    let mut c = Controls::new();
    let (mut jumps, mut lands, mut crashes) = (0u32, 0u32, 0u32);
    let mut airborne_frames = 0u32;
    let mut spins = [0u32; 4];
    let mut prev = PikachuState::Riding;
    let mut max_speed = 0.0f32;
    let mut run_frames = 0u64;
    let mut swiped = false;

    for f in 0..80_000u64 {
        let airborne = prev == PikachuState::Jumping;
        match policy.as_str() {
            "oneflip" if airborne && !swiped => {
                c.feed(Gesture::Swipe(Dir::Left));
                swiped = true;
            }
            "spin" if airborne => c.feed(Gesture::Press(Dir::Left)),
            _ => {}
        }
        if !airborne {
            swiped = false;
            if policy == "spin" {
                c.feed(Gesture::Release);
            }
        }
        if g.in_results() || g.is_game_over() {
            if f % 8 == 0 {
                c.feed(Gesture::Tap);
            }
        } else {
            run_frames = f;
        }
        let meter = g.radness_meter();
        if g.step(c.frame()) == Status::Finished {
            break;
        }
        let s = g.state();
        if s != prev {
            match s {
                PikachuState::Jumping => jumps += 1,
                PikachuState::Landing => {
                    lands += 1;
                    spins[(meter as usize).min(3)] += 1;
                }
                PikachuState::Crashed => crashes += 1,
                _ => {}
            }
        }
        if s == PikachuState::Jumping {
            airborne_frames += 1;
        }
        max_speed = max_speed.max(g.speed_pixels());
        prev = s;
    }
    println!(
        "seed {seed:<5} {policy:<8} {:.1}s  section {:>2}/24  total {:>4}  radness {:>4}  \
         jumps {jumps:>2} lands {lands:>2} wipeouts {crashes:>2}  air {:.0}%  \
         landed-with-spins {:?}  max {max_speed:.2}px/f  {}",
        run_frames as f32 / 59.73,
        g.section(),
        g.total_value(),
        g.radness_value(),
        airborne_frames as f32 * 100.0 / run_frames.max(1) as f32,
        &spins[1..],
        if g.is_game_over() { "GAME OVER" } else { "finished" },
    );
}
