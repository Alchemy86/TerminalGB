//! Frontends. **Nothing here is part of the emulator core** — every module is
//! behind a cargo feature so an embedder can take the machine without a window
//! system, a terminal or an image codec. See README "Embedding the core".

#[cfg(target_arch = "wasm32")]
pub mod web;

/// OpenGL desktop window. Requires the `desktop` feature (glutin + gl).
#[cfg(all(not(target_arch = "wasm32"), feature = "desktop"))]
pub mod desktop;

/// Frontend config file (`config.toml`) + recent-ROM list. Dependency-free,
/// but it reads `$HOME`/`$XDG_*`, so it is frontend furniture, not core.
#[cfg(not(target_arch = "wasm32"))]
pub mod config;

#[cfg(not(target_arch = "wasm32"))]
pub mod frame;

#[cfg(not(target_arch = "wasm32"))]
pub mod romlib;

/// What the terminal itself costs per displayed frame — see [`termcost`].
#[cfg(not(target_arch = "wasm32"))]
pub mod termcost;

/// Which graphics protocol the terminal actually speaks — see [`termcaps`].
/// Pure: builds the query, parses the reply, opens no file descriptor.
#[cfg(not(target_arch = "wasm32"))]
pub mod termcaps;

/// Terminal frontend. Requires the `tui` feature (ratatui + ratatui-image).
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub mod tui;

/// Layout of the live Gen 1 trainer panel — see [`trainer_view`].
/// Requires the `tui` feature: it renders ratatui widgets.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub mod trainer_view;

/// Layout of the AI agent's commentary panel — see [`agent_view`].
/// Requires the `tui` feature: it renders ratatui widgets.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub mod agent_view;
