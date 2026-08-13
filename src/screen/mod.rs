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
