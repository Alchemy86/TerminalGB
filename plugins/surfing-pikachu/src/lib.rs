//! Surfing Pikachu, ported to Rust and playable in a terminal.
//!
//! A standalone reimplementation of the Pokémon Yellow minigame: the state
//! machine, the wave generator, the jump arc, the scoring and the raster
//! effect, rebuilt from pret/pokeyellow's verified disassembly. It needs no
//! ROM and no save file — [`game::Game`] is the whole machine, and
//! [`render`] draws what the Game Boy would have drawn.
//!
//! Input is gestures, not buttons — the target is a touch screen. See
//! [`input`] for the mapping and for what the original's controls really are.
//!
//! ```no_run
//! use surfing_pikachu::{game::{Game, Status}, input::{Controls, Gesture, Dir},
//!                       render, tiles::TileSet};
//! let (tiles, _warning) = TileSet::load(None);
//! let mut game = Game::new(0xc0ffee);
//! let mut controls = Controls::new();
//! let mut frame = vec![0u8; render::FRAME_BYTES];
//! controls.feed(Gesture::Swipe(Dir::Left));       // one spin of the board
//! while game.step(controls.frame()) == Status::Running {
//!     game.render(&tiles, &mut frame);
//! }
//! ```
//!
//! See this crate's `README.md` for what was ported, what was substituted and
//! why, and `REFERENCE.md` for the citations.

pub mod bcd;
pub mod fixed;
pub mod game;
pub mod input;
pub mod render;
pub mod rng;
pub mod tiles;
pub mod wave;

pub use game::{Game, Status};
pub use input::{Controls, Dir, Gesture};
