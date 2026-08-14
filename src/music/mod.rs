//! **Music mode** — a shelf of cartridges presented like albums: the game's own
//! artwork, its own title, its own music, and a key that writes what you are
//! hearing to a WAV.
//!
//! # What is easy here, and what is not
//!
//! * **The title is free.** Every cartridge carries it at header `$0134`.
//! * **Cover art is very doable.** Boot the ROM, let it reach its title screen,
//!   keep the frame. It is the game's own artwork, drawn by the game. Titles
//!   animate in, so [`cover`] keeps several candidates and the shelf lets you
//!   cycle them rather than guessing once and being wrong for ever.
//! * **Track selection is the hard part and has no general solution.** A Game
//!   Boy cartridge exposes no "play track 5" interface — there is no standard
//!   sound-driver entry point, no track table, nothing. What this module does is
//!   the one approach that works for every cartridge without knowing anything
//!   about it: **play what the game plays.** Boot it and listen. That reliably
//!   gets you the title theme and nothing else.
//!
//! [`TrackSource`] exists so the shelf can *say* which of those it got, because
//! "title theme" and "full soundtrack" are very different promises. Today every
//! album is [`TrackSource::Boot`]. Two richer sources are named there rather
//! than implemented, with an honest note about what each would cost.
//!
//! # Copyright
//!
//! This operates on ROMs **the user supplies**, and renders art and audio from
//! those files **at run time**. That is what every emulator does. Nothing
//! derived from a commercial cartridge is ever written into this repository —
//! not artwork, not audio, not screenshots, not test fixtures. The fixtures in
//! [`testrom`] are a few dozen bytes of our own machine code precisely so that
//! the tests, the demo shelf and CI never need a game.
//!
//! # Where it sits
//!
//! Music mode is a **feature, not core emulation**: everything here is behind
//! the `music` cargo feature, so `--no-default-features` (the embedding build)
//! carries none of it. The WAV encoder is not ours — it belongs to
//! `terminalgb-chiptune`, which the chiptune renderer also uses, so there is
//! exactly one of it.

pub mod capture;
pub mod cover;
pub mod library;
pub mod testrom;

pub use library::{Album, AudioVerdict, Library, TrackSource};
