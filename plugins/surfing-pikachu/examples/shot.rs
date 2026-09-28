//! Render one frame to the terminal as text, so the picture can be checked
//! without a terminal session. `shot <frame> [seed] [gfx-dir]`.
use surfing_pikachu::game::{Game, Input, PikachuState};
use surfing_pikachu::render::{FRAME_BYTES, SCREEN_H, SCREEN_W};
use surfing_pikachu::tiles::TileSet;

fn main() {
    let mut a = std::env::args().skip(1);
    // `<frames>` or a name: run until the game is doing that thing.
    let until = a.next().unwrap_or_else(|| "300".into());
    let seed: u32 = a.next().and_then(|s| s.parse().ok()).unwrap_or(1);
    let gfx = a.next().filter(|s| s != "-").map(std::path::PathBuf::from);
    let spin = a.next().is_some();
    let (tiles, warn) = TileSet::load(gfx.as_deref());
    if let Some(w) = warn {
        eprintln!("{w}");
    }
    eprintln!("{}", tiles.source.describe());
    let mut g = Game::new(seed);
    let mut buf = vec![0u8; FRAME_BYTES];
    let frames: Option<u64> = until.parse().ok();
    let mut at = 0u64;
    while at < 60_000 {
        let reached = match (frames, until.as_str()) {
            (Some(n), _) => at >= n,
            (_, "air") => g.state() == PikachuState::Jumping && g.radness_meter() > 0,
            (_, "crash") => g.state() == PikachuState::Crashed,
            (_, "results") => g.showing_results_screen() && g.routine() >= 7,
            (_, "tally") => (8..0x0c).contains(&g.routine()),
            (_, "gameover") => g.is_game_over(),
            _ => true,
        };
        if reached {
            break;
        }
        // Hold a spin while airborne so the jump frames show a rotated board.
        let airborne = spin && g.state() == PikachuState::Jumping;
        let confirm = g.is_game_over() && g.frame_count() % 8 == 0;
        g.step(Input {
            left: airborne,
            a: confirm,
            ..Default::default()
        });
        at += 1;
    }
    g.render(&tiles, &mut buf);
    // Two scanlines per text row, four shades, so the eye can read the wave.
    let ramp = [' ', '-', '+', '#'];
    for y in (0..SCREEN_H).step_by(2) {
        let mut line = String::with_capacity(SCREEN_W);
        for x in 0..SCREEN_W {
            let v = buf[(y * SCREEN_W + x) * 4];
            line.push(
                ramp[match v {
                    255 => 0,
                    192 => 1,
                    96 => 2,
                    _ => 3,
                }],
            );
        }
        println!("{line}");
    }
    eprintln!(
        "frame {at} seed {seed} routine {:#04x} results-map {} state {:?} hp {} section {} speed {:.2}",
        g.routine(),
        g.showing_results_screen(),
        g.state(),
        g.hp_value(),
        g.section(),
        g.speed_pixels()
    );
}
