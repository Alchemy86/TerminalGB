//! Frontends. **Nothing here is part of the emulator core** — every module is
//! behind a cargo feature so an embedder can take the machine without a window
//! system, a terminal or an image codec. See README "Embedding the core".

#[cfg(target_arch = "wasm32")]
pub mod web;

/// Web Audio playback — see [`webaudio`]. The browser's answer to
/// `sound::cpal_audio`; the reusable half lives in `sound::web_audio`.
#[cfg(all(target_arch = "wasm32", not(feature = "ffi")))]
pub mod webaudio;

/// Browser frame pacing — see [`pace`]. Pure arithmetic over a clock the
/// caller supplies, so `requestAnimationFrame`'s rate cannot become the
/// emulator's. Compiled under `cfg(test)` on the host so it has real tests.
#[cfg(any(target_arch = "wasm32", test))]
pub mod pace;

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

/// Crisp `▀` rendering for a terminal with no image protocol — see
/// [`halfblocks`]. Pure: samples a framebuffer into a `ratatui` buffer and
/// touches nothing else.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub mod halfblocks;

/// Semantic colour and the shared panel layout primitives — see
/// [`panel_theme`]. Deliberately **not** behind `plugin-trainer`: it is the one
/// place a colour is given a meaning, and every panel this frontend draws reads
/// it. Requires the `tui` feature only because it speaks in ratatui styles.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub mod panel_theme;

/// Layout of the live Gen 1 trainer panel — see [`trainer_view`].
/// Requires the `tui` feature: it renders ratatui widgets.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
#[cfg(feature = "plugin-trainer")]
pub mod trainer_view;

/// Layout and wording of the music-mode shelf — see [`music_view`].
/// Requires the `tui` feature: it renders ratatui widgets.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui", feature = "music"))]
pub mod music_view;

/// Layout of the AI agent's commentary panel — see [`agent_view`].
/// Requires the `tui` feature: it renders ratatui widgets.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
#[cfg(feature = "plugin-agent")]
pub mod agent_view;
