//! A Game Boy / Game Boy Color emulator core, plus the frontends built on it.
//!
//! # Embedding
//!
//! `--no-default-features` gives you the **core alone**: no window system, no
//! terminal, no image codecs, no host audio. The only third-party crate left
//! is `blip_buf` (the APU's band-limited resampler). The whole embedding API
//! is [`terminalgb::Gameboy`]:
//!
//! ```no_run
//! let rom: Vec<u8> = std::fs::read("game.gb").unwrap();
//! // `None` = do not touch the filesystem for battery saves.
//! let mut gb = terminalgb::gameboy::Gameboy::new(rom, None);
//! gb.frame();                       // run one ~59.7275 Hz frame
//! let rgba: &[u8] = gb.image();     // 160 x 144 RGBA
//! gb.keydown(terminalgb::KeypadKey::A);
//! let snapshot = gb.save_state();
//! ```
//!
//! Everything else is opt-in:
//!
//! | feature   | brings in                              | needs at run time     |
//! |-----------|----------------------------------------|-----------------------|
//! | `desktop` | `glutin`, `gl`, `libc`, `image`        | a display server      |
//! | `tui`     | `ratatui`, `ratatui-image`, `image`    | a kitty-protocol term |
//! | `audio`   | `cpal`                                 | a sound device        |
//! | `mcp`     | — (nothing)                            | an MCP client         |
//! | `ffi`     | —                                      | —                     |
//!
//! The core reads no environment variables, opens no device and assumes no
//! `$HOME`. Host assumptions live in [`screen`] (config/state paths),
//! [`video`] (raw video-out) and [`sound::cpal_audio`] (playback).
//! See the README's "Embedding the core" section for the honest portability
//! list.

use std::sync::Mutex;
use std::sync::OnceLock;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::*;

/// Render chiptune instructions through the emulated APU — see [`chiptune`].
/// Behind the `chiptune` feature; the music side of it lives in the
/// dependency-free `terminalgb-chiptune` crate, which does not depend on this one.
/// BGB's link-cable protocol over TCP — see [`bgblink`]. Behind the `bgblink`
/// feature (in `default`, out of `--no-default-features`) and adds **no**
/// third-party crate: it is `std::net` and eight bytes at a time. Excluded on
/// `wasm32`, which has no sockets.
#[cfg(all(feature = "bgblink", not(target_arch = "wasm32")))]
pub mod bgblink;
pub mod boot;
#[cfg(all(feature = "chiptune", not(target_arch = "wasm32")))]
pub mod chiptune;
pub mod cpu;
/// Minimal DEFLATE compressor for terminal transmission — see [`deflate`].
pub mod deflate;
pub mod gameboy;
/// Reading a **real** Generation 1 battle out of a running cartridge, so the
/// model in `terminalgb-gen1-battle` can be held to it — see [`gen1battle`].
///
/// Not compiled on wasm: it exists to drive a headless differential run against
/// the player's own cartridge, which a browser build has no business doing.
#[cfg(all(feature = "gen1-battle", not(target_arch = "wasm32")))]
pub mod gen1battle;
mod gpu;
mod input;
/// Two consoles, one wire — see [`link::LinkCable`].
pub mod link;
/// The World Trade Centre's relay client (see `docs/worldlobby.md`).
#[cfg(feature = "plugin-gen1-lobby")]
pub mod lobby;
/// **The long-horizon runner** — an agent plays unattended for hours at
/// headless speed and reports in. See [`longplay`] and `docs/longplay.md`.
/// Behind the `longplay` feature (which is `plugin-agent` plus this module) and
/// adds **no third-party crate**: the JSON codec is the agent plugin's and the
/// ntfy push is delegated to the system `curl`, the same bar `portal` meets.
#[cfg(all(feature = "longplay", not(target_arch = "wasm32")))]
pub mod longplay;
mod mbc;
/// Model Context Protocol server — drive the emulator from any MCP client.
/// See [`mcp`]. Behind the `mcp` feature and adds no third-party crate.
#[cfg(all(feature = "mcp", not(target_arch = "wasm32")))]
pub mod mcp;
mod mmu;
pub mod mode;
/// **Music mode** — cartridges as albums, with cover art and audio taken from
/// the game itself at run time. See [`music`]. A feature, not core emulation:
/// behind the `music` cargo feature, so an embedding build carries none of it.
#[cfg(all(feature = "music", not(target_arch = "wasm32")))]
pub mod music;
/// **Scenarios** — a declarative goal, rules the host *enforces*, the knowledge
/// an agent may plan against, and a run recorded as a file that replays exactly
/// with no model calls. See [`scenario`] and `docs/scenarios.md`. Behind the
/// `scenario` feature (`longplay` plus this module) and adds **no third-party
/// crate**.
#[cfg(all(feature = "scenario", not(target_arch = "wasm32")))]
pub mod scenario;
/// The PPU's public surface: which of the two rendering engines runs.
///
/// The `gpu` module itself stays private — its internals are not an API — but
/// [`terminalgb::Gameboy::set_render_mode`](crate::gameboy::Gameboy::set_render_mode)
/// takes this type, so an embedder has to be able to name it. Deliberately
/// **not** re-exported at the crate root: [`crate::gameboy::RenderMode`] is a
/// different enum (which *frontend* to run) and two types of the same name in
/// one namespace would be a trap.
pub mod ppu {
    pub use crate::gpu::RenderMode;
}
/// Runtime plugins — the *attach* half of the extension surface. The contract
/// itself is the dependency-free [`terminalgb_plugin_api`] crate; this module
/// re-exports it and implements the emulator's side. See [`plugin`].
pub mod plugin;
/// Trade-portal client: a network [`PokemonSource`] and the device-code
/// sign-in. Behind the `portal` cargo feature and never on wasm, so the
/// embedding build inherits no network code. See [`portal`] and
/// `docs/portal.md`.
///
/// [`PokemonSource`]: terminalgb_plugin_gen1_link::spec::PokemonSource
#[cfg(all(feature = "portal", not(target_arch = "wasm32"), not(feature = "ffi")))]
pub mod portal;
/// Measurement-only instrumentation; see [`profile`]. Behind the `profile`
/// cargo feature, off by default and in no shipping build.
#[cfg(feature = "profile")]
pub mod profile;
pub mod savestate;
#[cfg(not(feature = "ffi"))]
pub mod screen;
pub mod sgb;
/// Shared-memory frame handoff for the Kitty protocol — see [`shmframe`].
/// Compiles everywhere; the module itself stubs out where shared memory does
/// not exist, so callers need no `cfg` of their own.
pub mod shmframe;
/// The APU — see [`sound`]. Compiled on **every** target, `wasm32` included:
/// a machine with no sound chip is not a quiet Game Boy, it is a different
/// machine (`NR52` reads back powered off, the frame sequencer never advances,
/// PCM12/PCM34 report nothing), and cartridges can see the difference. Host
/// *playback* is what varies: `sound::cpal_audio` behind the `audio` feature
/// on a native target, [`sound::web_audio`] on `wasm32`,
/// [`sound::SilentPlayer`] when there is no device at all.
pub mod sound;
/// Machine-readable run telemetry (NDJSON) — see [`telemetry`].
#[cfg(not(target_arch = "wasm32"))]
pub mod telemetry;
/// Capped-resolution terminal image transmission — see [`termimage`].
pub mod termimage;
/// Test support for plugin authors — see [`testing`]. Off by default.
#[cfg(feature = "testing")]
pub mod testing;
pub mod upscale;
/// Raw video-out: the picture as a stream other tools consume — see [`video`].
#[cfg(not(target_arch = "wasm32"))]
pub mod video;

pub use crate::input::KeypadKey;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen(start)]
pub async fn start() {
    // screen::web::render().await
    // TODO: Process GL
}

static GAMEBOY: OnceLock<Mutex<Option<crate::gameboy::Gameboy>>> = OnceLock::new();

#[cfg(target_vendor = "apple")]
unsafe impl Send for crate::gameboy::Gameboy {}
#[cfg(target_vendor = "apple")]
unsafe impl Sync for crate::gameboy::Gameboy {}

/// Build the machine from a cartridge image.
///
/// `None` is the save path, and it means **only** "do not battery-back
/// cartridge RAM to a file" — an FFI host has nowhere to put one. It does not
/// choose the mapper: that comes from cartridge header byte `$0147`
/// ([`crate::mbc::get_mbc`]). It used to, and every FFI and web caller was
/// handed an MBC3 whatever the cartridge said; `lib::ffi_tests` drives this
/// entry point and checks it.
///
/// # Safety
///
/// This function is not safe due to from_raw_parts.
#[no_mangle]
pub unsafe extern "C" fn load(bytes: *const std::ffi::c_uchar, bytes_length: usize) {
    let bytes = std::slice::from_raw_parts(bytes, bytes_length);
    let bytes: Vec<u8> = Vec::from(bytes);
    GAMEBOY
        .get_or_init(|| Some(crate::gameboy::Gameboy::new(bytes.to_vec(), None)).into());
}

#[no_mangle]
pub extern "C" fn frame() {
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(mut locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_mut() {
                gameboy.frame();
            };
        }
    }
}

#[no_mangle]
pub extern "C" fn keydown(key: KeypadKey) {
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(mut locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_mut() {
                gameboy.keydown(key);
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn keyup(key: KeypadKey) {
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(mut locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_mut() {
                gameboy.keyup(key);
            }
        }
    }
}

#[repr(C)]
pub struct ImageBuffer {
    len: i32,
    data: *const u8,
}

#[no_mangle]
pub extern "C" fn image() -> ImageBuffer {
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_ref() {
                let image: &[u8] = gameboy.image();
                let data = image.as_ptr();
                let len = image.len() as i32;
                // std::mem::forget(image);
                // My guess image will be dropped but let's test

                return ImageBuffer { len, data };
            }
        }
    }

    ImageBuffer {
        len: 0,
        data: std::ptr::null_mut(),
    }
}

/// Holds the most recent serialised save state so the returned pointer stays
/// valid until the next call (mirrors how `image()` hands out a borrowed ptr).
static STATE_BUF: OnceLock<Mutex<Vec<u8>>> = OnceLock::new();

/// Serialise the running machine. The returned [`ImageBuffer`] points at bytes
/// owned by [`STATE_BUF`]; copy them out before the next `save_state` call. The
/// browser can persist these to local storage or offer them as a download.
#[no_mangle]
pub extern "C" fn save_state() -> ImageBuffer {
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_ref() {
                let bytes = gameboy.save_state();
                let buf = STATE_BUF.get_or_init(|| Mutex::new(Vec::new()));
                if let Ok(mut b) = buf.lock() {
                    *b = bytes;
                    return ImageBuffer {
                        len: b.len() as i32,
                        data: b.as_ptr(),
                    };
                }
            }
        }
    }
    ImageBuffer {
        len: 0,
        data: std::ptr::null(),
    }
}

/// Restore a machine state produced by [`save_state`]. Returns `0` on success,
/// `-1` if no machine is loaded, `-2` if the state is invalid (bad magic,
/// version, or wrong game).
///
/// # Safety
///
/// Not safe due to `from_raw_parts`.
#[no_mangle]
pub unsafe extern "C" fn load_state(
    bytes: *const std::ffi::c_uchar,
    bytes_length: usize,
) -> i32 {
    let slice = std::slice::from_raw_parts(bytes, bytes_length);
    if let Some(gb) = GAMEBOY.get() {
        if let Ok(mut locked_gb) = gb.lock() {
            if let Some(gameboy) = locked_gb.as_mut() {
                return match gameboy.load_state(slice) {
                    Ok(()) => 0,
                    Err(_) => -2,
                };
            }
        }
    }
    -1
}

#[no_mangle]
#[cfg(feature = "ffi-base64")]
pub extern "C" fn image_base64() -> *const std::os::raw::c_char {
    use base64::{engine::general_purpose, Engine};
    use image::DynamicImage;
    use std::ffi::CString;
    use std::io::Cursor;

    if let Some(gb) = GAMEBOY.get() {
        if let Ok(mut locked_gb) = gb.lock() {
            let width = 160;
            let height = 144;

            let image: &[u8] = locked_gb.as_mut().unwrap().image();
            // std::mem::forget(image);

            // Allocate a new buffer for the RGB image, 3 bytes per pixel
            let mut output_data = vec![0u8; width as usize * height as usize * 3];

            let mut i = 0;
            // Iterate through 4-byte chunks of the image data (RGBA bytes)
            for chunk in image.chunks(4) {
                // ... and copy each of them to output, leaving out the A byte
                output_data[i..i + 3].copy_from_slice(&chunk[0..3]);
                i += 3;
            }

            let buffer =
                image::ImageBuffer::from_raw(width, height, output_data).unwrap();
            let img: DynamicImage = image::DynamicImage::ImageRgb8(buffer);
            // if scale > 1 {
            //     buffer = image::imageops::resize(
            //         &buffer,
            //         width * (scale as u32),
            //         height * (scale as u32),
            //         image::imageops::FilterType::Nearest,
            //     );
            // }

            let mut png: Vec<u8> = vec![];
            img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .expect("don't fail img write_to");
            let data = general_purpose::STANDARD.encode(&png);

            // let cstring_data = CString::new(img.as_bytes()).expect("don't fail");
            // return cstring_data.into_raw();

            // let mut png: Vec<u8> = vec![];
            // img.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png).expect("don't fail img write_to");
            // let data = general_purpose::STANDARD.encode(&png);

            let cstring_data = CString::new(data).expect("don't fail");
            return cstring_data.into_raw();
        }
    }

    let data = CString::new("").expect("don't fail");
    data.into_raw()
}

#[cfg(test)]
mod ffi_tests {
    use super::*;

    /// A cartridge whose entry code writes `$22` to the ROM bank register and
    /// then loops. Bank *n* is filled with the byte *n*, so which mapper is
    /// really installed is readable off the selected bank number.
    ///
    /// 32 banks = 512 KiB deliberately: the MBC1 multicart heuristic
    /// (`src/mbc/mbc1.rs`) only looks at cartridges of exactly 1 MiB, and a
    /// multicart wires BANK2 two bits lower, which would be a second reason
    /// for the bank to differ.
    fn bank_switching_cart(kind: u8) -> Vec<u8> {
        const BANKS: usize = 32;
        let mut rom = Vec::with_capacity(BANKS * 0x4000);
        for b in 0..BANKS {
            rom.extend(std::iter::repeat_n(b as u8, 0x4000));
        }
        rom[0x147] = kind;
        rom[0x148] = 4; // 32 banks
        rom[0x149] = 0; // no cartridge RAM
                        // $0100: ld a,$22 / ld [$2000],a / jr -2
        rom[0x100..0x107].copy_from_slice(&[0x3E, 0x22, 0xEA, 0x00, 0x20, 0x18, 0xFE]);
        rom
    }

    /// **The FFI entry point picks the mapper from the cartridge header.**
    ///
    /// Symptom when it does not: a non-MBC3 game embedded through the C ABI
    /// (or the web build, which constructs the machine the same way) reads the
    /// wrong 16 KiB ROM bank from its very first bank switch — garbage code,
    /// an immediate freeze or crash — while the identical ROM is fine in the
    /// desktop build, which passes a save path. Nothing in the test-ROM suite
    /// catches it: Blargg, Mooneye and acid2 are all driven with a path, so
    /// they always took the correct branch.
    ///
    /// This drives `load` — the real `extern "C"` symbol — rather than
    /// re-deriving the construction, because "the core is right" and "the
    /// entry point is right" are different claims. `GAMEBOY` is a `OnceLock`,
    /// so exactly one test in this process may use it.
    #[test]
    fn the_ffi_entry_point_takes_the_mapper_from_the_cartridge_header() {
        // MBC1 masks the bank register to five bits, so $22 selects bank 2.
        // MBC3 uses seven and would select bank $22 (34).
        let rom = bank_switching_cart(0x01);
        unsafe { load(rom.as_ptr(), rom.len()) };
        frame();

        let gb = GAMEBOY.get().expect("load() built a machine");
        let locked = gb.lock().expect("machine lock");
        let gb = locked.as_ref().expect("machine present");
        assert_eq!(
            gb.debug_rom_bank(),
            2,
            "an MBC1 cartridge loaded over the FFI must bank like an MBC1; \
             34 means the MBC3 fallback is back"
        );
    }
}
