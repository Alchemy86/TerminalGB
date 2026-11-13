#[cfg(target_arch = "wasm32")]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub mod desktop;

#[cfg(not(target_arch = "wasm32"))]
pub mod config;

#[cfg(not(target_arch = "wasm32"))]
pub mod frame;

#[cfg(not(target_arch = "wasm32"))]
pub mod romlib;

#[cfg(not(target_arch = "wasm32"))]
pub mod tui;

/// Layout of the live Gen 1 trainer panel — see [`trainer_view`].
#[cfg(not(target_arch = "wasm32"))]
pub mod trainer_view;
