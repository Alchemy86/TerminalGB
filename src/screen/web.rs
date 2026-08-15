#![cfg(not(feature = "ffi"))]

extern crate console_error_panic_hook;

use crate::gameboy::Gameboy;
use crate::input::KeypadKey;

use core::cell::{RefCell, RefMut};
use std::rc::Rc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

use web_sys::{CanvasRenderingContext2d, ImageData};

use std::panic;

// #[wasm_bindgen]
// extern "C" {
//     #[wasm_bindgen(js_namespace = console)]
//     fn log(s: String);
//     #[wasm_bindgen(js_namespace = console, js_name = log)]
//     fn log_u32(a: u32);
// }

fn window() -> web_sys::Window {
    web_sys::window().expect("no global `window` exists")
}

#[allow(unused)]
fn add_event_listener(listener: &str, f: &Closure<dyn FnMut(web_sys::KeyboardEvent)>) {
    window()
        .add_event_listener_with_callback(listener, f.as_ref().unchecked_ref())
        .expect("should register `add_event_listener` OK");
}

fn request_animation_frame(f: &Closure<dyn FnMut(f64)>) {
    window()
        .request_animation_frame(f.as_ref().unchecked_ref())
        .expect("should register `requestAnimationFrame` OK");
}

thread_local! {
    /// One line of live run state, refreshed about once a second by the frame
    /// loop and readable from the page through [`status`].
    ///
    /// The terminal frontend has a Status panel; the browser had nothing at
    /// all, so a silent `AudioContext` (the browser's autoplay policy is the
    /// usual reason), a mis-negotiated sample rate or a starved audio queue
    /// were indistinguishable from "the emulator is broken". Same rule as the
    /// TUI's `Plugin`/`Audio`/`Term` rows: a state nobody can see is a state
    /// nobody can debug.
    static STATUS: RefCell<String> = const { RefCell::new(String::new()) };
}

/// A one-line summary of what the emulator and its audio path are doing.
///
/// Empty before the first frame. The demo page polls this; so can anything
/// else embedding the module.
#[wasm_bindgen]
pub fn status() -> String {
    STATUS.with(|s| s.borrow().clone())
}

// TODO: Move to WebGL tex2d
#[wasm_bindgen]
pub async fn render(rom: Vec<u8>) -> Result<(), JsValue> {
    panic::set_hook(Box::new(console_error_panic_hook::hook));

    let document = web_sys::window().unwrap().document().unwrap();
    let game = document.get_element_by_id("game");

    let canvas = document
        .create_element("canvas")?
        .dyn_into::<web_sys::HtmlCanvasElement>()?;
    game.unwrap().append_child(&canvas)?;
    canvas.set_width(160);
    canvas.set_height(144);

    let context = canvas
        .get_context("2d")
        .unwrap()
        .unwrap()
        .dyn_into::<CanvasRenderingContext2d>()
        .unwrap();

    // The mapper comes from cartridge header byte `$0147`; `None` only means
    // "do not battery-back cartridge RAM to a file", which a browser cannot do
    // anyway. See `crate::mbc::get_mbc` — this used to be an MBC3 whatever the
    // cartridge said, so every non-MBC3 game was mis-banked here.
    let mut gb = Gameboy::new(rom, None);

    // Audio. `render` is called from the page's Play button handler, so the
    // `AudioContext` is created inside a user gesture and is allowed to start.
    // A failure is reported and the emulator carries on: an APU is still
    // installed either way, because a machine with no sound chip is a
    // *different machine* (`NR52` reads back powered off, and cartridges spin
    // on it) — see `crate::sound::SilentPlayer`.
    let audio: Rc<RefCell<Option<crate::screen::webaudio::WebAudio>>> = Rc::new(
        RefCell::new(match crate::screen::webaudio::WebAudio::start(&mut gb) {
            Ok(a) => Some(a),
            Err(e) => {
                crate::sound::host_status::set_failed(format!("{e:?}"));
                None
            }
        }),
    );
    if audio.borrow().is_none() {
        gb.enable_audio(Box::new(crate::sound::SilentPlayer));
    }

    let f = Rc::new(RefCell::new(None));
    let g = f.clone();
    // let mut i = 0;
    let current_key_code: Rc<RefCell<i32>> = Rc::new(RefCell::new(0));
    // Pixel-art upscaler (see crate::upscale). Starts as a 1x no-op; `]`
    // (keycode 221) cycles the algorithm live, `\` (220) the scale factor.
    // Shared with the keydown listener so cycling happens once per press, not
    // once per animation frame.
    let upscaler = Rc::new(RefCell::new(crate::upscale::Upscaler::new(
        crate::upscale::Algorithm::Nearest,
        1,
    )));
    gb.frame();

    {
        let current_key_code = current_key_code.clone();
        let upscaler = upscaler.clone();
        let canvas = canvas.clone();
        let audio = audio.clone();
        // Emulation is paced on real time, never on the animation frame's
        // rate — see `crate::screen::pace`, which is where the 120 Hz
        // double-speed bug is pinned. The clock is the *audio* clock when
        // there is one, so producer and consumer cannot drift apart.
        let mut pacer = crate::screen::pace::FramePacer::new();
        // Status accounting: emulated frames since the last report, and when
        // that report was.
        let mut stat_frames: u32 = 0;
        let mut stat_at: Option<f64> = None;
        *g.borrow_mut() = Some(Closure::wrap(Box::new(move |timestamp_ms: f64| {
            // if i >= 10 {
            //     let _ = f.borrow_mut().take();
            //     return;
            // }

            // i += 1;

            let key: RefMut<_> = current_key_code.borrow_mut();

            // log_u32(*key);
            match *key {
                // A
                65 => gb.keydown(KeypadKey::A),
                -65 => gb.keyup(KeypadKey::A),
                // S
                83 => gb.keydown(KeypadKey::B),
                -83 => gb.keyup(KeypadKey::B),
                // Z
                90 => gb.keydown(KeypadKey::Select),
                -90 => gb.keyup(KeypadKey::Select),
                // X
                88 => gb.keydown(KeypadKey::Start),
                -88 => gb.keyup(KeypadKey::Start),

                // Left
                37 => gb.keydown(KeypadKey::Left),
                -37 => gb.keyup(KeypadKey::Left),
                // Right
                39 => gb.keydown(KeypadKey::Right),
                -39 => gb.keyup(KeypadKey::Right),
                // Up
                38 => gb.keydown(KeypadKey::Up),
                -38 => gb.keyup(KeypadKey::Up),
                // Down
                40 => gb.keydown(KeypadKey::Down),
                -40 => gb.keyup(KeypadKey::Down),
                _ => (),
            }

            let mut audio_ref = audio.borrow_mut();
            let now = match audio_ref.as_ref() {
                Some(a) => a.now(),
                None => timestamp_ms / 1000.0,
            };
            let due = pacer.frames_due(now);
            for _ in 0..due {
                gb.frame();
            }
            // Service audio on every callback, including the ones that stepped
            // nothing: on a display faster than 59.7275 Hz most callbacks are
            // due zero frames, and the scheduler still has to top the cushion
            // up.
            if let Some(a) = audio_ref.as_mut() {
                a.pump();
            }
            stat_frames += due;
            let since = *stat_at.get_or_insert(now);
            if now - since >= 1.0 {
                let fps = f64::from(stat_frames) / (now - since);
                let line = match audio_ref.as_ref() {
                    Some(a) => {
                        let st = a.stats();
                        let (depth, drops) = a.queue_depth_and_drops();
                        format!(
                            "{fps:.1} fps · audio {} {} Hz · lead {:.0} ms · \
                             queue {depth} · underruns {} · drops {drops}",
                            if a.running() { "live" } else { "suspended" },
                            a.sample_rate(),
                            a.lead() * 1000.0,
                            st.underruns,
                        )
                    }
                    None => format!(
                        "{fps:.1} fps · audio {}",
                        crate::sound::host_status::snapshot().label()
                    ),
                };
                STATUS.with(|s| *s.borrow_mut() = line);
                stat_frames = 0;
                stat_at = Some(now);
            }
            drop(audio_ref);
            if due == 0 {
                // Nothing new to show. Drawing the same frame again costs a
                // full `putImageData` for no change.
                request_animation_frame(f.borrow().as_ref().unwrap());
                return;
            }

            let mut up = upscaler.borrow_mut();
            let (ow, oh) = up.output_size(160, 144);
            // Keep the canvas sized to the upscaled frame (the algorithm can
            // change between frames via the hotkeys).
            if canvas.width() != ow as u32 || canvas.height() != oh as u32 {
                canvas.set_width(ow as u32);
                canvas.set_height(oh as u32);
            }
            let scaled: &[u8] = up.apply(gb.image(), 160, 144);
            match ImageData::new_with_u8_clamped_array_and_sh(
                wasm_bindgen::Clamped(scaled),
                ow as u32,
                oh as u32,
            ) {
                Ok(d) => {
                    context.put_image_data(&d, 0.0, 0.0).ok();
                }
                Err(_err) => {
                    // log(format!("{:?}", _err));
                }
            };
            drop(up);

            request_animation_frame(f.borrow().as_ref().unwrap());
        }) as Box<dyn FnMut(f64)>));
    }
    {
        let current_key_code = current_key_code.clone();
        let upscaler = upscaler.clone();
        let audio_keys = audio.clone();
        let closure =
            Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
                let key = event.key_code();
                // `]` cycles the upscaling algorithm, `\` its factor — handled
                // here (once per press) rather than in the frame loop.
                // Any key press is a user gesture: if the browser suspended
                // the context (autoplay policy, or a backgrounded tab), this
                // is the moment it is allowed to start again.
                if let Some(a) = audio_keys.borrow().as_ref() {
                    a.nudge();
                }
                match key {
                    221 => upscaler.borrow_mut().cycle_algorithm(),
                    220 => upscaler.borrow_mut().cycle_factor(),
                    _ => *current_key_code.borrow_mut() = key as i32,
                }
            });
        window().add_event_listener_with_callback(
            "keydown",
            closure.as_ref().unchecked_ref(),
        )?;
        closure.forget();
    }
    {
        let current_key_code = current_key_code.clone();
        let closure =
            Closure::<dyn FnMut(_)>::new(move |event: web_sys::KeyboardEvent| {
                let key = event.key_code();
                *current_key_code.borrow_mut() = -(key as i32);
            });
        window().add_event_listener_with_callback(
            "keyup",
            closure.as_ref().unchecked_ref(),
        )?;
        closure.forget();
    }

    request_animation_frame(g.borrow().as_ref().unwrap());

    Ok(())
}

// pub fn compile_shader(
//     context: &WebGlRenderingContext,
//     shader_type: u32,
//     source: &str,
// ) -> Result<WebGlShader, String> {
//     let shader = context
//         .create_shader(shader_type)
//         .ok_or_else(|| String::from("Unable to create shader object"))?;
//     context.shader_source(&shader, source);
//     context.compile_shader(&shader);

//     if context
//         .get_shader_parameter(&shader, WebGlRenderingContext::COMPILE_STATUS)
//         .as_bool()
//         .unwrap_or(false)
//     {
//         Ok(shader)
//     } else {
//         Err(context
//             .get_shader_info_log(&shader)
//             .unwrap_or_else(|| String::from("Unknown error creating shader")))
//     }
// }

// pub fn link_program(
//     context: &WebGlRenderingContext,
//     vert_shader: &WebGlShader,
//     frag_shader: &WebGlShader,
// ) -> Result<WebGlProgram, String> {
//     let program = context
//         .create_program()
//         .ok_or_else(|| String::from("Unable to create shader object"))?;

//     context.attach_shader(&program, vert_shader);
//     context.attach_shader(&program, frag_shader);
//     context.link_program(&program);

//     if context
//         .get_program_parameter(&program, WebGlRenderingContext::LINK_STATUS)
//         .as_bool()
//         .unwrap_or(false)
//     {
//         Ok(program)
//     } else {
//         Err(context
//             .get_program_info_log(&program)
//             .unwrap_or_else(|| String::from("Unknown error creating program object")))
//     }
// }
