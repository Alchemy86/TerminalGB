extern crate gl;
extern crate glutin;
extern crate libc;

use crate::gameboy::Gameboy;
use crate::input::KeypadKey;

use std::ffi::CString;
use std::iter::repeat;
use std::mem;
use std::ptr;
use std::str;

use gl::types::*;
use glutin::event::{ElementState, VirtualKeyCode};

/// The window title's **stable prefix**.
///
/// Automated runners find the window by title — the GB Emulator Shootout's
/// default check is `title.startswith(<emulator name>)` — so the prefix is a
/// published fact, not decoration. Everything the frontend appends (the
/// upscaler's name, the volume) comes after it, and this is the one place it
/// is written down.
pub const WINDOW_TITLE: &str = "TerminalGB";

/// Command-line options for the OpenGL window frontend.
///
/// These exist because an *automated* runner needs the three things a human
/// never asks for: a specific console, a window of a known size showing the
/// raw frame, and a way to run faster than real time. The GB Emulator
/// Shootout's runner is the concrete case — it screenshots the client area and
/// compares it against a 160×144 reference — but the same three answer
/// "record a clip", "script a run" and "compare two builds by eye", so they
/// are ordinary flags rather than a special mode. See `docs/shootout.md`.
///
/// Defaults reproduce the frontend's historical behaviour exactly: model from
/// the cartridge header, window sized to the configured upscaler's output,
/// real time.
#[derive(Debug, Clone, PartialEq)]
pub struct DesktopOptions {
    /// Which console to boot. Applied by the caller at construction time
    /// (`Gameboy::new_with_model`), not by the window.
    pub model: crate::mode::Model,
    /// Client area, in whole multiples of the native 160×144 frame. `None`
    /// keeps the historical behaviour: the window is sized to the upscaler's
    /// output and stays resizable. `Some(n)` fixes it at `n*160 × n*144` and
    /// makes it non-resizable, so nothing — a window manager, a tiling rule,
    /// a stray drag — can change what a screenshot captures.
    pub window_scale: Option<u32>,
    /// Override the config file's `upscale` key. `--upscale none` is what
    /// puts the untouched 160×144 frame on screen whatever the user's
    /// `config.toml` says.
    pub upscale: Option<crate::upscale::Algorithm>,
    /// Override the config file's `upscale_factor` key.
    pub upscale_factor: Option<u8>,
    /// Emulation speed as a multiple of real time. 1.0 is a true DMG's
    /// 59.7275 Hz. Above 1 the *emulator* runs faster; the window still draws
    /// at most 59.7275 times a second, so the extra frames are emulated in a
    /// burst rather than drawn (the same trade the terminal frontend's speed
    /// ladder makes).
    pub speed: f64,
    /// Keep emulating while the window does not have keyboard focus.
    ///
    /// The frontend's default is to pause — reasonable for a person, and
    /// **fatal for anything automated**: a runner that launches the emulator
    /// from a script may never hand it focus (Windows refuses focus changes
    /// requested by a background process), and a screenshot of a paused window
    /// is a screenshot of the title screen. The pause is not observable from
    /// the outside, so the failure looks like an emulation bug.
    pub run_unfocused: bool,
}

impl Default for DesktopOptions {
    fn default() -> Self {
        DesktopOptions {
            model: crate::mode::Model::Auto,
            window_scale: None,
            upscale: None,
            upscale_factor: None,
            speed: 1.0,
            run_unfocused: false,
        }
    }
}

/// The most the emulator will run faster than real time. Not a policy so much
/// as a guard: `--speed` multiplies how many frames one draw may emulate, and
/// an unbounded value would let a typo park the process in a loop that never
/// returns to the event queue.
pub const MAX_SPEED: f64 = 100.0;

impl DesktopOptions {
    /// Pull the frontend's flags out of `args`, returning them and whatever is
    /// left (the ROM path, and anything a caller parses itself).
    ///
    /// Unknown `--flags` are **left in the remainder** rather than rejected:
    /// this parser runs after `crate::video::apply_cli_flags`, and a caller may
    /// well have flags of its own. The ROM path is whatever survives.
    pub fn parse<I: IntoIterator<Item = String>>(
        args: I,
    ) -> Result<(DesktopOptions, Vec<String>), String> {
        let mut opts = DesktopOptions::default();
        let mut rest = Vec::new();
        let mut it = args.into_iter().peekable();
        while let Some(arg) = it.next() {
            // `--flag=value` and `--flag value` are both accepted; a runner's
            // argv is generated, and both shapes are generated in the wild.
            let (name, inline) = match arg.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (arg.clone(), None),
            };
            let mut take = |what: &str| -> Result<String, String> {
                match inline.clone().or_else(|| it.next()) {
                    Some(v) => Ok(v),
                    None => Err(format!("{name} needs a {what}")),
                }
            };
            match name.as_str() {
                "--model" => {
                    let v = take("console (auto|dmg|cgb|sgb|sgb2)")?;
                    opts.model = crate::mode::Model::parse(&v).ok_or_else(|| {
                        format!("--model {v:?} is not one of auto/dmg/cgb/sgb/sgb2")
                    })?;
                }
                "--scale" | "--window-scale" => {
                    let v = take("whole number")?;
                    let n: u32 = v
                        .parse()
                        .map_err(|_| format!("--scale {v:?} is not a number"))?;
                    if n == 0 || n > 16 {
                        return Err(format!("--scale {n} is outside 1..=16"));
                    }
                    opts.window_scale = Some(n);
                }
                "--upscale" => {
                    let v = take("algorithm")?;
                    opts.upscale = Some(
                        crate::upscale::Algorithm::from_str(&v).ok_or_else(|| {
                            format!(
                                "--upscale {v:?} is not one of \
                                     none/nearest/scale2x/mmpx/lcd"
                            )
                        })?,
                    );
                }
                "--upscale-factor" => {
                    let v = take("whole number")?;
                    let n: u8 = v
                        .parse()
                        .map_err(|_| format!("--upscale-factor {v:?} is not a number"))?;
                    opts.upscale_factor = Some(n);
                }
                "--speed" => {
                    let v = take("multiplier")?;
                    let s: f64 = v
                        .parse()
                        .map_err(|_| format!("--speed {v:?} is not a number"))?;
                    if !(s.is_finite() && s > 0.0 && s <= MAX_SPEED) {
                        return Err(format!("--speed {v} is outside 0..{MAX_SPEED}"));
                    }
                    opts.speed = s;
                }
                "--run-unfocused" => opts.run_unfocused = true,
                _ => rest.push(arg),
            }
        }
        Ok((opts, rest))
    }

    /// `--help` text for the flags this struct owns.
    pub fn usage() -> &'static str {
        "  --model <console>         auto (default), dmg, cgb, sgb, sgb2\n  \
         --scale <N>               fixed client area of N x 160 x N x 144 \
         (default: fit the upscaler)\n  \
         --upscale <algo>          none, scale2x, mmpx, lcd — overrides the \
         config file\n  \
         --upscale-factor <N>      overrides the config file\n  \
         --speed <N>               emulation speed, 1 = real time (e.g. 15 for \
         turbo)\n  \
         --run-unfocused           keep emulating when the window is not \
         focused (default: pause)"
    }
}

pub struct Glcx {
    #[allow(unused)]
    tex: GLuint,
    #[allow(unused)]
    program: GLuint,
    #[allow(unused)]
    frag: GLuint,
    #[allow(unused)]
    vert: GLuint,
    #[allow(unused)]
    ebo: GLuint,
    #[allow(unused)]
    vbo: GLuint,
    #[allow(unused)]
    vao: GLuint,
}

// pub fn render(gameboy: &mut Gameboy) {
//     let mut ratio = 1 + (gameboy.height / 10);
//     let event_loop: glutin::event_loop::EventLoop<()> =
//         glutin::event_loop::EventLoop::with_user_event();
//     let inner_size = glutin::dpi::LogicalSize {
//         width: gameboy.width,
//         height: gameboy.height,
//     };
//     let window_builder = glutin::window::WindowBuilder::new()
//         .with_title("LR35902")
//         .with_inner_size(inner_size)
//         .with_resizable(false);
//     let gl_window = glutin::ContextBuilder::new()
//         .build_windowed(window_builder, &event_loop)
//         .unwrap();
//     let gl_window = unsafe { gl_window.make_current().unwrap() };

//     gl::load_with(|s| gl_window.get_proc_address(s) as *const _);

//     let cx = Glcx::new();
//     let mut focused = true;
//     event_loop.run(move |event, _, control_flow| {
//         let window = gl_window.window();
//         match event {
//             glutin::event::Event::WindowEvent {
//                 window_id: _,
//                 event: wevent,
//             } => *control_flow = process_window(window, &wevent, &mut gameboy, &mut focused),
//             glutin::event::Event::MainEventsCleared => window.request_redraw(),
//             glutin::event::Event::RedrawRequested(_) => {
//                 if focused == true {
//                     gameboy.frame();
//                     cx.draw(gameboy.width, gameboy.height, gameboy.image());
//                     gl_window.swap_buffers().unwrap();
//                 }

//                 thread::sleep(Duration::from_millis(10));
//             }
//             _ => {
//                 let next_frame_time =
//                     std::time::Instant::now() + std::time::Duration::from_nanos(16_666_667);
//                 *control_flow = glutin::event_loop::ControlFlow::WaitUntil(next_frame_time);
//             }
//         }
//     })
// }

pub fn process_window(
    _window: &glutin::window::Window,
    wevent: &glutin::event::WindowEvent,
    gameboy: &mut Gameboy,
    focused: &mut bool,
) -> glutin::event_loop::ControlFlow {
    match wevent {
        glutin::event::WindowEvent::Focused(f) => {
            *focused = *f;
            glutin::event_loop::ControlFlow::Poll
        }
        glutin::event::WindowEvent::KeyboardInput { input, .. } => {
            if let Some(virt_keycode) = input.virtual_keycode {
                let button = match virt_keycode {
                    VirtualKeyCode::A => KeypadKey::A,
                    VirtualKeyCode::B => KeypadKey::B,
                    VirtualKeyCode::Z => KeypadKey::Select,
                    VirtualKeyCode::X => KeypadKey::Start,

                    VirtualKeyCode::Left => KeypadKey::Left,
                    VirtualKeyCode::Right => KeypadKey::Right,
                    VirtualKeyCode::Down => KeypadKey::Down,
                    VirtualKeyCode::Up => KeypadKey::Up,

                    _ => return glutin::event_loop::ControlFlow::Poll,
                };
                match input.state {
                    ElementState::Pressed => gameboy.keydown(button),
                    ElementState::Released => gameboy.keyup(button),
                }
            }

            glutin::event_loop::ControlFlow::Poll
        }
        glutin::event::WindowEvent::Resized(glutin::dpi::PhysicalSize {
            width: _,
            height: _,
        }) => glutin::event_loop::ControlFlow::Poll,
        glutin::event::WindowEvent::CloseRequested => {
            // winit's event loop calls `process::exit` on Exit, so nothing here
            // is ever dropped — the battery save has to be written now.
            gameboy.flush_battery();
            glutin::event_loop::ControlFlow::Exit
        }
        _ => glutin::event_loop::ControlFlow::Poll,
    }
}

// Shader sources
const VERTEX: &str = r"#version 150 core
in vec2 position;
in vec3 color;
in vec2 texcoord;
out vec3 Color;
out vec2 Texcoord;
void main() {
   Color = color;
   Texcoord = texcoord;
   gl_Position = vec4(position, 0.0, 1.0);
}
";

const FRAGMENT: &str = r"#version 150 core
in vec3 Color;
in vec2 Texcoord;
out vec4 outColor;
uniform sampler2D tex;
void main() {
   outColor = texture(tex, Texcoord);
}
";

/// Compute the centred, integer-scaled OpenGL viewport rectangle
/// `(x, y, w, h)` in framebuffer pixels for a `tex_w`×`tex_h` source drawn
/// inside a `fb_w`×`fb_h` framebuffer.
///
/// The largest integer N for which `tex_w*N × tex_h*N` still fits the
/// framebuffer is chosen, so every Game Boy pixel maps to the *same* whole
/// number of screen pixels (no shimmer from uneven pixel sizes) and the 10:9
/// aspect ratio is preserved. The rectangle is centred; the leftover border is
/// letterboxed by the caller's clear.
///
/// **N = 0 handling:** when the framebuffer is smaller than the source in
/// either axis, integer division would give N = 0. We clamp to **N = 1** and
/// centre, so the image is shown at 1× and cropped by the window edges rather
/// than dividing to zero or vanishing. `x`/`y` may therefore be negative — that
/// is deliberate (it centres the crop) and is exactly what `glViewport` expects.
pub fn integer_scale_viewport(
    tex_w: u32,
    tex_h: u32,
    fb_w: u32,
    fb_h: u32,
) -> (i32, i32, i32, i32) {
    // Degenerate source: nothing sensible to scale; fill the framebuffer.
    if tex_w == 0 || tex_h == 0 {
        return (0, 0, fb_w as i32, fb_h as i32);
    }

    let scale = std::cmp::min(fb_w / tex_w, fb_h / tex_h).max(1);
    let w = (tex_w * scale) as i32;
    let h = (tex_h * scale) as i32;
    let x = (fb_w as i32 - w) / 2;
    let y = (fb_h as i32 - h) / 2;
    (x, y, w, h)
}

impl Glcx {
    /// Update the GL viewport to a centred, integer-scaled rectangle for the
    /// given framebuffer size. Called on the `Resized` window event (and each
    /// frame in `draw`) so enlarging the window actually re-fills it.
    pub fn resize(&self, tex_w: u32, tex_h: u32, fb_w: u32, fb_h: u32) {
        let (x, y, w, h) = integer_scale_viewport(tex_w, tex_h, fb_w, fb_h);
        unsafe {
            gl::Viewport(x, y, w, h);
        }
    }

    // No `Default`: constructing this issues GL calls, so it is only valid
    // once a context is current — exactly what a `Default::default()` would
    // invite someone to get wrong.
    #[allow(clippy::new_without_default)]
    pub fn new() -> Glcx {
        unsafe {
            let mut vao = 0;
            gl::GenVertexArrays(1, &mut vao);
            gl::BindVertexArray(vao);

            let mut vbo = 0;
            gl::GenBuffers(1, &mut vbo);

            const VERTICES: &[f32] = &[
                //  Position   Color             Texcoords
                -1.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, // Top-left
                1.0, 1.0, 0.0, 1.0, 0.0, 1.0, 0.0, // Top-right
                1.0, -1.0, 0.0, 0.0, 1.0, 1.0, 1.0, // Bottom-right
                -1.0, -1.0, 1.0, 1.0, 1.0, 0.0, 1.0, // Bottom-left
            ];
            gl::BindBuffer(gl::ARRAY_BUFFER, vbo);
            gl::BufferData(
                gl::ARRAY_BUFFER,
                (VERTICES.len() * 4) as libc::ssize_t,
                VERTICES.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            let mut ebo = 0;
            gl::GenBuffers(1, &mut ebo);

            const ELEMENTS: &[GLuint] = &[0, 1, 2, 2, 3, 0];

            gl::BindBuffer(gl::ELEMENT_ARRAY_BUFFER, ebo);
            gl::BufferData(
                gl::ELEMENT_ARRAY_BUFFER,
                std::mem::size_of_val(ELEMENTS) as libc::ssize_t,
                ELEMENTS.as_ptr() as *const _,
                gl::STATIC_DRAW,
            );

            let vert = gl::CreateShader(gl::VERTEX_SHADER);
            let src = CString::new(VERTEX).unwrap();
            gl::ShaderSource(vert, 1, &src.as_ptr(), std::ptr::null::<i32>());
            gl::CompileShader(vert);
            // Glcx::check_shader_compile(&gl, vert);

            // Create and compile the fragment shader
            let frag = gl::CreateShader(gl::FRAGMENT_SHADER);
            let src = CString::new(FRAGMENT).unwrap();
            gl::ShaderSource(frag, 1, &src.as_ptr(), std::ptr::null::<i32>());
            gl::CompileShader(frag);
            // TODO: check_shader_compile(frag)

            let program = gl::CreateProgram();
            gl::AttachShader(program, vert);
            gl::AttachShader(program, frag);
            let buf = CString::new("outColor").unwrap();
            gl::BindFragDataLocation(program, 0, buf.as_ptr());
            gl::LinkProgram(program);
            // TODO: check_program_link(program)
            assert_eq!(gl::GetError(), 0);
            gl::UseProgram(program);

            let buf = CString::new("position").unwrap();
            let pos_attrib = gl::GetAttribLocation(program, buf.as_ptr());
            gl::EnableVertexAttribArray(pos_attrib as u32);
            gl::VertexAttribPointer(
                pos_attrib as u32,
                2,
                gl::FLOAT,
                gl::FALSE,
                (7 * mem::size_of::<GLfloat>()) as i32,
                std::ptr::null(),
            );

            let buf = CString::new("color").unwrap();
            let col_attrib = gl::GetAttribLocation(program, buf.as_ptr());
            gl::EnableVertexAttribArray(col_attrib as u32);
            gl::VertexAttribPointer(
                col_attrib as u32,
                3,
                gl::FLOAT,
                gl::FALSE,
                (7 * mem::size_of::<GLfloat>()) as i32,
                (2 * mem::size_of::<GLfloat>()) as *const _,
            );

            let buf = CString::new("texcoord").unwrap();
            let tex_attrib = gl::GetAttribLocation(program, buf.as_ptr());
            gl::EnableVertexAttribArray(tex_attrib as u32);
            gl::VertexAttribPointer(
                tex_attrib as u32,
                2,
                gl::FLOAT,
                gl::FALSE,
                (7 * mem::size_of::<GLfloat>()) as i32,
                (5 * mem::size_of::<GLfloat>()) as *const _,
            );

            // Load textures
            let mut tex = 0;
            gl::GenTextures(1, &mut tex);

            gl::ActiveTexture(gl::TEXTURE0);
            gl::BindTexture(gl::TEXTURE_2D, tex);
            let buf = CString::new("tex").unwrap();
            gl::Uniform1i(gl::GetUniformLocation(program, buf.as_ptr()), 0);

            gl::TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_S,
                gl::CLAMP_TO_EDGE as i32,
            );
            gl::TexParameteri(
                gl::TEXTURE_2D,
                gl::TEXTURE_WRAP_T,
                gl::CLAMP_TO_EDGE as i32,
            );
            // NEAREST (not LINEAR): the Game Boy screen is a 160x144 grid of
            // hard-edged pixels. Bilinear filtering smears each pixel into its
            // neighbours when the image is enlarged, which looks blurry on pixel
            // art. NEAREST keeps every source pixel an exact square.
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::NEAREST as i32);
            gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::NEAREST as i32);

            Glcx {
                tex,
                program,
                frag,
                vert,
                ebo,
                vbo,
                vao,
            }
        }
    }

    #[allow(dead_code)]
    unsafe fn check_program_link(_gl: &Glcx, program: GLuint) {
        let mut status = gl::FALSE as GLint;
        gl::GetProgramiv(program, gl::LINK_STATUS, &mut status);
        if status == (gl::TRUE as GLint) {
            return;
        }

        let mut len: GLint = 0;
        gl::GetProgramiv(program, gl::INFO_LOG_LENGTH, &mut len);
        let mut buf = repeat(0u8).take(len as usize).collect::<Vec<_>>();
        gl::GetProgramInfoLog(
            program,
            len,
            ptr::null_mut(),
            buf.as_mut_ptr() as *mut GLchar,
        );
        panic!("{}", str::from_utf8(&buf).unwrap());
    }

    /// Draw the `width`×`height` frame `data` into a `fb_width`×`fb_height`
    /// framebuffer, integer-scaled and centred with a black letterbox border.
    pub fn draw(
        &self,
        width: u32,
        height: u32,
        fb_width: u32,
        fb_height: u32,
        data: &[u8],
    ) {
        unsafe {
            // Black border for the letterbox. glClear ignores the viewport (it
            // clears the whole framebuffer since scissor test is off), so this
            // paints the border first; the scaled quad is then drawn on top
            // inside the integer-scaled viewport.
            gl::ClearColor(0.0, 0.0, 0.0, 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT);

            // Recompute the integer-scaled, centred viewport every frame so the
            // image tracks the current window size (and any DPI change).
            let (vx, vy, vw, vh) =
                integer_scale_viewport(width, height, fb_width, fb_height);
            gl::Viewport(vx, vy, vw, vh);

            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGB as i32,
                width as i32,
                height as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                data.as_ptr() as *const _,
            );
            assert_eq!(gl::GetError(), 0);

            gl::DrawElements(gl::TRIANGLES, 6, gl::UNSIGNED_INT, std::ptr::null());
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use crate::mode::Model;
    use crate::upscale::Algorithm;

    fn parse(s: &str) -> Result<(DesktopOptions, Vec<String>), String> {
        DesktopOptions::parse(s.split_whitespace().map(str::to_string))
    }

    /// No flags must mean exactly what the frontend did before they existed:
    /// console from the header, window sized to the upscaler, real time.
    #[test]
    fn the_defaults_are_the_old_behaviour() {
        let (opts, rest) = parse("game.gb").unwrap();
        assert_eq!(opts, DesktopOptions::default());
        assert_eq!(opts.model, Model::Auto);
        assert_eq!(opts.window_scale, None);
        assert_eq!(opts.upscale, None);
        assert_eq!(opts.speed, 1.0);
        assert_eq!(rest, vec!["game.gb"]);
    }

    /// The combination a screenshot-comparing conformance runner passes: a
    /// known console, a client area of exactly 160x144, the untouched frame in
    /// it, and enough speed that a few hundred test ROMs finish in CI.
    #[test]
    fn a_conformance_runner_gets_a_known_picture_of_a_known_machine() {
        let (opts, rest) =
            parse("--model dmg --scale 1 --upscale none --speed 15 rom.gb").unwrap();
        assert_eq!(opts.model, Model::Dmg);
        assert_eq!(opts.window_scale, Some(1));
        // `none` is the config file's own spelling of "do not upscale", and it
        // has to override a `config.toml` that says otherwise — that file is
        // the reason a flag is needed at all.
        assert_eq!(opts.upscale, Some(Algorithm::Nearest));
        assert_eq!(opts.speed, 15.0);
        assert_eq!(rest, vec!["rom.gb"]);
    }

    /// Every console the shootout's manifest asks for is reachable by name.
    #[test]
    fn every_model_the_runner_asks_for_can_be_selected() {
        for (flag, want) in [
            ("dmg", Model::Dmg),
            ("cgb", Model::Cgb),
            ("sgb", Model::Sgb),
            ("sgb2", Model::Sgb2),
            ("auto", Model::Auto),
        ] {
            let (opts, _) = parse(&format!("--model {flag} r.gb")).unwrap();
            assert_eq!(opts.model, want, "--model {flag}");
        }
    }

    /// A generated argv may use either shape; both are accepted.
    #[test]
    fn inline_and_separated_values_both_parse() {
        let a = parse("--scale=2 --speed=4 r.gb").unwrap().0;
        let b = parse("--scale 2 --speed 4 r.gb").unwrap().0;
        assert_eq!(a, b);
        assert_eq!(a.window_scale, Some(2));
    }

    /// A typo must be refused, never silently ignored: a run that quietly
    /// booted the wrong console would produce a wrong score with no sign of it.
    #[test]
    fn a_bad_value_is_refused_rather_than_ignored() {
        assert!(parse("--model dgm r.gb").is_err());
        assert!(parse("--scale 0 r.gb").is_err());
        assert!(parse("--scale 99 r.gb").is_err());
        assert!(parse("--speed 0 r.gb").is_err());
        assert!(parse("--speed -1 r.gb").is_err());
        assert!(parse("--speed nan r.gb").is_err());
        assert!(parse("--upscale sparkle r.gb").is_err());
        assert!(parse("--model").is_err());
    }

    /// This parser runs after `video::apply_cli_flags` and before the caller
    /// picks the ROM path, so anything it does not own must survive untouched.
    #[test]
    fn flags_this_parser_does_not_own_are_passed_through() {
        let (opts, rest) = parse("--scale 3 --something-else r.gb").unwrap();
        assert_eq!(opts.window_scale, Some(3));
        assert_eq!(rest, vec!["--something-else", "r.gb"]);
    }

    /// The window title's prefix is what an automated runner matches on, so it
    /// is a published fact — pinned here so a rename is a deliberate act.
    #[test]
    fn the_window_title_prefix_is_stable() {
        assert_eq!(WINDOW_TITLE, "TerminalGB");
        // Whatever the frontend appends still starts with the prefix.
        let live = format!("{WINDOW_TITLE} — nearest x1 — vol 50%");
        assert!(live.starts_with(WINDOW_TITLE));
    }
}

#[cfg(test)]
mod tests {
    use super::integer_scale_viewport;

    // The Game Boy source resolution used throughout.
    const GB_W: u32 = 160;
    const GB_H: u32 = 144;

    /// Every returned rectangle must be a whole-number multiple of the source
    /// (so pixels stay even) and centred (borders on opposite sides differ by
    /// at most 1 from integer division).
    fn assert_integer_and_centred(tex_w: u32, tex_h: u32, fb_w: u32, fb_h: u32) {
        let (x, y, w, h) = integer_scale_viewport(tex_w, tex_h, fb_w, fb_h);
        assert_eq!(w % tex_w as i32, 0, "width {w} not a multiple of {tex_w}");
        assert_eq!(h % tex_h as i32, 0, "height {h} not a multiple of {tex_h}");
        assert_eq!(
            w / tex_w as i32,
            h / tex_h as i32,
            "aspect ratio not preserved (unequal scale on the two axes)"
        );
        // Centred: leading border equals trailing border within rounding.
        let right = fb_w as i32 - (x + w);
        let bottom = fb_h as i32 - (y + h);
        assert!(
            (x - right).abs() <= 1,
            "not horizontally centred: {x} vs {right}"
        );
        assert!(
            (y - bottom).abs() <= 1,
            "not vertically centred: {y} vs {bottom}"
        );
    }

    #[test]
    fn exact_multiple_fills_completely() {
        // 2x: 320x288 fits exactly, no border.
        assert_eq!(
            integer_scale_viewport(GB_W, GB_H, 320, 288),
            (0, 0, 320, 288)
        );
        // 4x.
        assert_eq!(
            integer_scale_viewport(GB_W, GB_H, 640, 576),
            (0, 0, 640, 576)
        );
    }

    #[test]
    fn picks_largest_fitting_integer() {
        // 700x600: width allows 4x (640), height allows 4x (576) -> 4x.
        assert_eq!(
            integer_scale_viewport(GB_W, GB_H, 700, 600),
            (30, 12, 640, 576)
        );
        // Height is the limiting axis: 1000x300 -> height/144 = 2 -> 2x.
        let (_, _, w, h) = integer_scale_viewport(GB_W, GB_H, 1000, 300);
        assert_eq!((w, h), (320, 288));
    }

    #[test]
    fn non_integer_framebuffer_letterboxes() {
        // 2.5x worth of space: must snap DOWN to 2x and centre, never 2.5x.
        assert_integer_and_centred(GB_W, GB_H, 400, 360);
        let (x, y, w, h) = integer_scale_viewport(GB_W, GB_H, 400, 360);
        assert_eq!((w, h), (320, 288), "must snap to 2x, not stretch to 2.5x");
        assert_eq!((x, y), (40, 36), "leftover space must be centred");
        // Odd/awkward sizes stay integer + centred.
        assert_integer_and_centred(GB_W, GB_H, 483, 291);
        assert_integer_and_centred(GB_W, GB_H, 1000, 1000);
        assert_integer_and_centred(GB_W, GB_H, 161, 145);
    }

    #[test]
    fn window_smaller_than_source_clamps_to_1x_and_crops() {
        // N would be 0; we clamp to 1x. Rect is source-sized and centred, so
        // x/y go negative -> the window edges crop it. Never zero-sized.
        let (x, y, w, h) = integer_scale_viewport(GB_W, GB_H, 100, 100);
        assert_eq!((w, h), (160, 144), "must clamp to 1x, not 0x");
        assert!(w > 0 && h > 0, "viewport must never collapse to zero");
        assert_eq!(
            (x, y),
            (-30, -22),
            "1x crop must be centred (negative offset)"
        );

        // Zero-sized framebuffer (e.g. minimised window) still yields 1x, not a
        // divide-by-zero or empty rect.
        let (_, _, w0, h0) = integer_scale_viewport(GB_W, GB_H, 0, 0);
        assert_eq!((w0, h0), (160, 144));
    }

    #[test]
    fn degenerate_source_fills_framebuffer() {
        // A zero-dimension source can't be scaled; fall back to filling the fb
        // rather than dividing by zero.
        assert_eq!(integer_scale_viewport(0, 144, 640, 480), (0, 0, 640, 480));
        assert_eq!(integer_scale_viewport(160, 0, 640, 480), (0, 0, 640, 480));
        assert_eq!(integer_scale_viewport(0, 0, 640, 480), (0, 0, 640, 480));
    }
}
