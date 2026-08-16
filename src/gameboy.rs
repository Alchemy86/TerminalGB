use crate::cpu::core::Cpu;
use crate::input::KeypadKey;

pub struct Gameboy {
    cpu: Cpu<'static>,
    pub width: u32,
    pub height: u32,
    /// Bytes written into the machine from outside the game this session. See
    /// [`Gameboy::host_writes`]; not serialised, deliberately.
    host_writes: u64,
}

pub use self::Target::{GameBoy, GameBoyColor, SuperGameBoy};

#[derive(PartialEq, Eq, Debug, Copy, Clone, Default)]
pub enum RenderMode {
    /// No frontend at all: the embedder drives [`Gameboy::frame`] itself and
    /// reads [`Gameboy::image`]. Always available, and the default — a build
    /// with `--no-default-features` has no other option.
    #[default]
    Headless,
    /// Requires the `desktop` feature.
    #[cfg(all(feature = "desktop", not(any(target_arch = "wasm32", feature = "ffi"))))]
    Desktop,
    /// Requires the `tui` feature.
    #[cfg(all(feature = "tui", not(any(target_arch = "wasm32", feature = "ffi"))))]
    Terminal,
    #[cfg(any(target_arch = "wasm32", feature = "ffi"))]
    WebAssembly,
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum Target {
    GameBoy,
    GameBoyColor,
    SuperGameBoy,
}

#[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
pub fn load_rom(filepath: &str) -> Result<(Vec<u8>, std::path::PathBuf), String> {
    use std::fs::File;
    use std::io::Read;

    let mut rom = Vec::new();
    if filepath.is_empty() {
        return Err(String::from("Please provide a valid filepath"));
    }

    let file = File::open(filepath);
    match file.and_then(|mut f| f.read_to_end(&mut rom)) {
        Ok(_) => {}
        Err(e) => return Err(format!("Failed to read {:?}: {}", filepath, e)),
    };

    Ok((rom, std::path::PathBuf::from(filepath)))
}

// Match rboy's timing: (4194304 / 1000 * 16) = ~67,109 cycles per 16ms frame
pub const CYCLES: u32 = 67109;

/// T-cycles in one DMG frame: 70,224 at 4.194304 MHz is 59.7275 Hz.
pub const FRAME_TICKS: u32 = 70224;

/// The most T-cycles [`Gameboy::frame`] will execute in one call.
///
/// A frame's worth plus a margin. The margin exists because a single
/// instruction can carry the GPU a few cycles past the frame boundary, so with
/// the LCD on the `updated` break always arrives first and this cap is dead
/// code — which is what makes it safe to apply unconditionally. With the LCD
/// off there is no `updated` break at all, and this is the only thing standing
/// between a frame-paced frontend and an unbounded run.
pub const FRAME_TICKS_CAP: u32 = FRAME_TICKS + 64;

/// Launch the terminal frontend. `initial_rom` starts straight into that game;
/// `None` opens the ROM picker. This is the entry point terminal binaries
/// should use — it lets the TUI own ROM loading, so the picker and in-app ROM
/// switching work.
#[cfg(all(feature = "tui", not(any(target_arch = "wasm32", feature = "ffi"))))]
pub fn run_terminal(initial_rom: Option<std::path::PathBuf>) {
    // Fail loudly (and non-zero) on setup errors — e.g. a terminal with no
    // kitty keyboard support — rather than silently returning.
    if let Err(e) = crate::screen::tui::run(initial_rom) {
        eprintln!("terminal frontend error: {e}");
        std::process::exit(1);
    }
}

/// Open the terminal frontend straight into **music mode** — the shelf of
/// cartridges-as-albums. See [`crate::music`] and `docs/music-mode.md`.
#[cfg(all(
    feature = "music",
    feature = "tui",
    not(any(target_arch = "wasm32", feature = "ffi"))
))]
pub fn run_music() {
    if let Err(e) = crate::screen::tui::run_as(crate::screen::tui::Start::Music) {
        eprintln!("terminal frontend error: {e}");
        std::process::exit(1);
    }
}

impl Gameboy {
    /// Build a machine from a cartridge image.
    ///
    /// `filepath` is the **only** thing that lets the emulator touch the
    /// filesystem: `Some(p)` opts into battery saves beside `p` (`<rom>.sav`),
    /// `None` keeps cartridge RAM entirely in memory. Nothing else here reads
    /// a path, a config file or `$HOME`.
    ///
    /// The console model is [`crate::mode::Model::Auto`], which consults
    /// `$GB_MODEL` before falling back to the cartridge header. An embedder
    /// that wants a machine independent of the ambient environment should use
    /// [`Gameboy::new_with_model`] instead.
    pub fn new(data: Vec<u8>, filepath: Option<std::path::PathBuf>) -> Gameboy {
        // let rom = load_rom();

        let gb = Gameboy {
            cpu: Cpu::new(data, filepath),
            width: 160,
            height: 144,
            host_writes: 0,
        };

        gb
    }

    /// [`Gameboy::new`] with the console chosen explicitly. An explicit model
    /// wins over `$GB_MODEL`, so this is the constructor to use when the run
    /// must be reproducible — a hardware build, a test rig, a library caller
    /// that cannot vouch for its process environment.
    pub fn new_with_model(
        data: Vec<u8>,
        filepath: Option<std::path::PathBuf>,
        model: crate::mode::Model,
    ) -> Gameboy {
        Gameboy {
            cpu: Cpu::new_with_model(data, filepath, model),
            width: 160,
            height: 144,
            host_writes: 0,
        }
    }

    /// [`Gameboy::new_with_model`] without the panic. Use this when the bytes
    /// came from somewhere you do not control — a file picker, a network
    /// upload, a user's SD card. `new`/`new_with_model` abort the process on a
    /// cartridge that cannot be mapped (unknown MBC type, an image too short to
    /// hold a header, an unreadable `.sav`), because `[profile.release]` sets
    /// `panic = 'abort'`.
    pub fn try_new_with_model(
        data: Vec<u8>,
        filepath: Option<std::path::PathBuf>,
        model: crate::mode::Model,
    ) -> Result<Gameboy, &'static str> {
        Ok(Gameboy {
            cpu: Cpu::try_new_with_model(data, filepath, model)?,
            width: 160,
            height: 144,
            host_writes: 0,
        })
    }

    /// [`Gameboy::new_with_model`] with an optional **user-supplied boot ROM**.
    ///
    /// `None` — the ordinary case, and what every checked-in baseline is
    /// measured on — is exactly [`Gameboy::new_with_model`]. `Some(rom)` maps
    /// the image over the cartridge and starts the machine at reset, so the
    /// handoff state (including video RAM, which cannot be reconstructed) is
    /// the real one. The frontend finds the file; see [`crate::boot`] for where
    /// it looks and why the core never does it itself.
    pub fn new_with_boot_rom(
        data: Vec<u8>,
        filepath: Option<std::path::PathBuf>,
        model: crate::mode::Model,
        boot: Option<crate::boot::BootRom>,
    ) -> Gameboy {
        Gameboy {
            cpu: Cpu::new_with_boot_rom(data, filepath, model, boot),
            width: 160,
            height: 144,
            host_writes: 0,
        }
    }

    /// Is a user-supplied boot ROM mapped (or still running)?
    pub fn boot_rom_mapped(&self) -> bool {
        self.cpu.memory.boot_rom_mapped()
    }

    pub fn render(self, render_mode: RenderMode) {
        match render_mode {
            // Nothing to do: the caller owns the loop.
            RenderMode::Headless => {}
            #[cfg(all(
                feature = "desktop",
                not(any(target_arch = "wasm32", feature = "ffi"))
            ))]
            RenderMode::Desktop => {
                self.render_desktop();
            }
            #[cfg(any(target_arch = "wasm32", feature = "ffi"))]
            RenderMode::WebAssembly => {
                // crate::screen::web::render();
            }
            #[cfg(all(
                feature = "tui",
                not(any(target_arch = "wasm32", feature = "ffi"))
            ))]
            RenderMode::Terminal => {
                self.render_terminal();
            }
        }
    }

    #[cfg(all(feature = "desktop", not(any(target_arch = "wasm32", feature = "ffi"))))]
    pub fn render_desktop(self) {
        self.render_desktop_with(crate::screen::desktop::DesktopOptions::default())
    }

    /// [`render_desktop`](Self::render_desktop) with the window and pacing
    /// chosen explicitly — see [`crate::screen::desktop::DesktopOptions`].
    ///
    /// `opts.model` is *not* read here: the console is decided when the machine
    /// is built (`Gameboy::new_with_model`), long before a window exists. It
    /// rides along in the struct so one parse produces every frontend option.
    #[cfg(all(feature = "desktop", not(any(target_arch = "wasm32", feature = "ffi"))))]
    pub fn render_desktop_with(mut self, opts: crate::screen::desktop::DesktopOptions) {
        use crate::screen::desktop::*;

        // Initialize audio (only when the `audio` feature is enabled; cpal pulls
        // in ALSA on Linux, so it stays opt-in to keep a fresh clone buildable).
        #[cfg(all(not(target_arch = "wasm32"), feature = "audio"))]
        let _audio_stream = match crate::sound::cpal_audio::CpalPlayer::new() {
            Some((player, stream)) => {
                self.enable_audio(Box::new(player));
                #[cfg(debug_assertions)]
                eprintln!("Audio enabled successfully");
                Some(stream)
            }
            None => {
                #[cfg(debug_assertions)]
                eprintln!("Failed to initialize audio");
                None
            }
        };
        #[cfg(any(target_arch = "wasm32", not(feature = "audio")))]
        let _audio_stream: Option<()> = None;

        // **A machine always has an APU, sound card or not.** `Sound::do_cycle`
        // returns immediately while no player is installed, so a build without
        // the `audio` feature — or a host where cpal found no device — was not
        // running a silent Game Boy but one with no APU at all: `NR52` reads
        // back powered off, the frame sequencer never advances, and PCM12/PCM34
        // report nothing. Cartridges can see that (`interrupt_time` spins on
        // `NR52`), so install a sink that throws the samples away rather than
        // leaving the chip out of the machine. See `crate::sound::SilentPlayer`.
        if !self.audio_enabled() {
            self.enable_audio(Box::new(crate::sound::SilentPlayer));
        }

        // Pixel-art upscaler (see crate::upscale): a CPU pre-pass on the
        // 160x144 frame before the texture upload. Chosen over a GL shader
        // because the same code then serves every frontend (terminal, wasm)
        // and the measured cost (<2 ms worst case, MMPX x4) is far inside the
        // 16.7 ms frame budget. Initial algorithm/factor come from the shared
        // config file; U cycles the algorithm live, I cycles the factor.
        let cfg = crate::screen::config::Config::load();
        // Playback volume: the `volume` config key is the starting level, and
        // whatever `,` / `.` / `m` last set is remembered across sessions (see
        // `crate::screen::config::VolumeState`). Host-side attenuation of the
        // finished sample stream only — never the emulated NR50.
        let volume_config = cfg.volume;
        {
            let (level, muted) =
                crate::screen::config::VolumeState::resolve(volume_config);
            crate::sound::output_volume::set_level(level);
            crate::sound::output_volume::set_muted(muted);
        }
        // The command line beats the config file, so a caller that needs a
        // known picture gets one whatever is in the user's `config.toml`.
        // `--upscale none` is the pairing with `--scale 1` that puts the
        // untouched 160x144 frame in a 160x144 client area.
        let algo = opts.upscale.unwrap_or(cfg.upscale);
        let factor = opts.upscale_factor.unwrap_or(cfg.upscale_factor);
        let mut upscaler = crate::upscale::Upscaler::new(algo, factor);
        let (mut out_w, mut out_h) =
            upscaler.output_size(self.width as usize, self.height as usize);

        // Raw video-out (see `crate::video`). Off unless configured **and**
        // explicitly started: naming a destination says where frames go, not
        // that they should be sent (`crate::video::resolve_autostart`). Unlike
        // the terminal frontend this one may stream to **stdout**, because
        // nothing else here writes there — so the documented pipe is:
        //   desktop game.gb --video-out - --video-start | ffmpeg -f rawvideo \
        //       -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - out.mp4
        let (video_spec, video_warnings) = crate::video::VideoSpec::resolve(&cfg.video);
        let (video_start, start_warnings) = crate::video::resolve_autostart(&cfg.video);
        for w in video_warnings.iter().chain(start_warnings.iter()) {
            eprintln!("terminalgb: {w}");
        }
        // This frontend has no key to turn the stream on, so an armed-but-idle
        // stream would be unreachable. Say what to add rather than leaving a
        // configured destination looking like a setting that was ignored.
        let video_spec = match video_spec {
            Some(s) if !video_start => {
                eprintln!(
                    "terminalgb: video: {} configured but not streaming — add \
                     --video-start (or video_autostart = true) to go live",
                    s.dest.label()
                );
                None
            }
            other => other,
        };
        let mut video = match video_spec {
            Some(mut s) => {
                // Fix the geometry from the configured upscaler so the size the
                // consumer is told is the truth from the first byte.
                if s.source == crate::video::Source::Display && s.size.is_none() {
                    s.size = Some((out_w as u32, out_h as u32));
                }
                // The game's name travels with the stream, so a FIFO's sidecar
                // descriptor can label the virtual camera after the cartridge.
                s.title = Some(self.rom_title());
                let (v, err) = crate::video::VideoOut::open(s);
                if let Some(e) = err {
                    eprintln!("terminalgb: {e}");
                }
                if v.is_enabled() {
                    eprintln!("terminalgb: {}", v.describe());
                }
                v
            }
            None => crate::video::VideoOut::disabled(),
        };
        let video_source = video.spec().source;

        /// Offer the just-emulated frame to the video stream. Called after
        /// **every** emulated frame (including catch-up ones) rather than per
        /// draw, so the stream's rate is the emulator's, not the window's.
        /// Free when video-out is off — the clock is not even read.
        fn feed_video(
            gb: &Gameboy,
            upscaler: &mut crate::upscale::Upscaler,
            video: &mut crate::video::VideoOut,
            source: crate::video::Source,
        ) {
            if !video.is_enabled() {
                // A held destination (a camera, a FIFO) keeps getting the idle
                // card while the game feed is off — see `VideoOut::go_idle`.
                if video.is_idle() {
                    video.idle(std::time::Instant::now());
                }
                return;
            }
            let now = std::time::Instant::now();
            match source {
                crate::video::Source::Native => {
                    video.frame(gb.image(), gb.width, gb.height, now)
                }
                crate::video::Source::Display => {
                    let (w, h) =
                        upscaler.output_size(gb.width as usize, gb.height as usize);
                    let scaled =
                        upscaler.apply(gb.image(), gb.width as usize, gb.height as usize);
                    video.frame(scaled, w as u32, h as u32, now);
                }
            }
        }

        let event_loop: glutin::event_loop::EventLoop<()> =
            glutin::event_loop::EventLoop::with_user_event();
        // `--scale N` fixes the client area at N x the *native* 160x144 frame,
        // in **device** pixels, and locks it. A screenshot-comparing runner
        // measures the client rect, so a logical size would be silently
        // multiplied by the display's scale factor, and a resizable window is
        // one a window manager may resize for its own reasons. Without the
        // flag this is exactly the old behaviour: a logical box the size of the
        // upscaler's output, resizable.
        let fixed = opts.window_scale;
        let window_builder = glutin::window::WindowBuilder::new()
            .with_title(WINDOW_TITLE)
            .with_resizable(fixed.is_none());
        let window_builder = match fixed {
            Some(n) => window_builder.with_inner_size(glutin::dpi::PhysicalSize {
                width: n * self.width,
                height: n * self.height,
            }),
            None => window_builder.with_inner_size(glutin::dpi::LogicalSize {
                width: out_w as u32,
                height: out_h as u32,
            }),
        };
        let gl_window = glutin::ContextBuilder::new()
            .build_windowed(window_builder, &event_loop)
            .unwrap();
        let gl_window = unsafe { gl_window.make_current().unwrap() };

        gl::load_with(|s| gl_window.get_proc_address(s) as *const _);

        let cx = Glcx::new();
        let mut focused = true;
        // `--run-unfocused` (see `DesktopOptions::run_unfocused`): an
        // automated runner may never be given focus at all, and a paused
        // emulator looks exactly like a broken one from the outside.
        let run_unfocused = opts.run_unfocused;
        // The DMG refreshes every 70224 CPU cycles at 4.194304 MHz = 59.7275 Hz,
        // i.e. one frame every 16.7427 ms. The old value (16 ms = 62.5 fps) paced
        // the whole emulator ~4.64% fast, which the player hears as the game (and
        // its music) running slightly fast AND as audio roughness: the APU then
        // produces ~4.6% more samples per wall-second than the sound card drains,
        // so the output queue overflows and drops samples periodically. Pacing at
        // the true frame period fixes both — see docs/audio.md.
        //
        // `--speed N` shortens the *emulated* frame period; the window still
        // draws at most 59.7275 times a second, so above 1x the extra frames
        // are emulated in a burst inside one draw rather than drawn. Drawing at
        // 15x would mean 900 texture uploads and buffer swaps a second, which
        // on a software OpenGL stack (the only kind a CI runner has) is what
        // would actually limit the speed. Same trade the terminal frontend's
        // ladder makes: emulated frames are never dropped, displayed ones are.
        const REAL_FRAME_US: f64 = 16_743.0; // 59.7275 Hz, a true DMG
        let frame_duration =
            std::time::Duration::from_micros((REAL_FRAME_US / opts.speed) as u64);
        // How many emulated frames one draw may run. At 1x this is the
        // historical 3 (absorbing OS jitter); above it, enough that a 59.73 Hz
        // draw loop can still keep the deadline — plus the same slack.
        let max_catchup = 3 + opts.speed.ceil() as u32;
        // Fast-forward tells the audio player to drop its overflow rather than
        // pace the emulator against the sound card (`docs/audio.md` §4c).
        if opts.speed > 1.0 {
            self.set_audio_fast_forward(true);
        }

        // Deadline accumulator, NOT `last = Instant::now()` after each frame.
        // Resetting to `now` adds the draw/scheduling overhead to every period,
        // so the emulator runs a few percent slow on average — and audio sample
        // production is slaved to emulation speed, so a slow producer drains
        // the audio queue and underruns ("blocky" sound). Advancing the
        // deadline by exactly one period keeps the long-run average at
        // 59.7275 Hz; a bounded catch-up absorbs late wakeups.
        let mut next_frame_time = std::time::Instant::now() + frame_duration;
        // The display's own deadline. At 1x it is the emulated frame period, so
        // the loop below is byte-for-byte the old one; above 1x it stays at
        // 59.7275 Hz while `frame_duration` shortens, which is what turns extra
        // speed into extra *emulated* frames instead of extra draws.
        let draw_interval =
            frame_duration.max(std::time::Duration::from_micros(REAL_FRAME_US as u64));
        let mut next_draw_time = std::time::Instant::now() + draw_interval;

        event_loop.run(move |event, _, control_flow| {
            let window = gl_window.window();
            match event {
                glutin::event::Event::WindowEvent {
                    window_id: _,
                    event: wevent,
                } => {
                    // Resizing must update the GL surface *and* the viewport, or
                    // the image stays pinned to the original 160x144 patch.
                    // (Both the surface resize and the gl::Viewport call live
                    // here because they need the GL context, which is current on
                    // this thread.) draw() re-applies the viewport every frame
                    // too, so this mainly makes the resize feel immediate.
                    if let glutin::event::WindowEvent::Resized(physical_size) = &wevent {
                        gl_window.resize(*physical_size);
                        cx.resize(
                            out_w as u32,
                            out_h as u32,
                            physical_size.width,
                            physical_size.height,
                        );
                        window.request_redraw();
                    }
                    // Upscaler hotkeys (handled here, not in process_window,
                    // because the upscaler lives in this closure): U cycles
                    // the algorithm, I the scale factor.
                    if let glutin::event::WindowEvent::KeyboardInput { input, .. } =
                        &wevent
                    {
                        if input.state == glutin::event::ElementState::Pressed {
                            use crate::sound::output_volume;
                            let mut volume_changed = false;
                            let changed = match input.virtual_keycode {
                                Some(glutin::event::VirtualKeyCode::U) => {
                                    upscaler.cycle_algorithm();
                                    true
                                }
                                Some(glutin::event::VirtualKeyCode::I) => {
                                    upscaler.cycle_factor();
                                    true
                                }
                                // Playback volume, the same keys the terminal
                                // frontend uses. Host-side attenuation only —
                                // see `crate::sound::output_volume`.
                                Some(glutin::event::VirtualKeyCode::Comma) => {
                                    output_volume::step(false);
                                    volume_changed = true;
                                    false
                                }
                                Some(glutin::event::VirtualKeyCode::Period) => {
                                    output_volume::step(true);
                                    volume_changed = true;
                                    false
                                }
                                Some(glutin::event::VirtualKeyCode::M) => {
                                    output_volume::toggle_mute();
                                    volume_changed = true;
                                    false
                                }
                                _ => false,
                            };
                            if volume_changed {
                                crate::screen::config::VolumeState::save(
                                    output_volume::level(),
                                    output_volume::muted(),
                                    volume_config,
                                );
                            }
                            if changed || volume_changed {
                                (out_w, out_h) = upscaler.output_size(
                                    self.width as usize,
                                    self.height as usize,
                                );
                                // The window title is this frontend's only
                                // status line, so the volume lives in it —
                                // a control nobody can see the state of is a
                                // control nobody trusts.
                                window.set_title(&format!(
                                    "{WINDOW_TITLE} — {} — vol {}",
                                    upscaler.label(),
                                    output_volume::label()
                                ));
                                window.request_redraw();
                            }
                        }
                    }
                    *control_flow =
                        process_window(window, &wevent, &mut self, &mut focused)
                }
                glutin::event::Event::MainEventsCleared => {
                    // Only request redraw if the display deadline has passed.
                    // (At 1x that is the frame deadline; above 1x the frames
                    // due in between are run inside the draw.)
                    if std::time::Instant::now() >= next_draw_time {
                        window.request_redraw();
                    }
                }
                glutin::event::Event::RedrawRequested(_) => {
                    if focused || run_unfocused {
                        self.frame();
                        feed_video(&self, &mut upscaler, &mut video, video_source);
                        next_frame_time += frame_duration;
                        // Late (slow draw, OS jitter)? Run extra emulated
                        // frames — bounded — to hold the average rate; if
                        // hopelessly behind (unfocus pause, suspend), resync
                        // instead of fast-forwarding.
                        let now = std::time::Instant::now();
                        if now > next_frame_time {
                            let mut extra = 0;
                            while now > next_frame_time && extra < max_catchup {
                                self.frame();
                                feed_video(
                                    &self,
                                    &mut upscaler,
                                    &mut video,
                                    video_source,
                                );
                                next_frame_time += frame_duration;
                                extra += 1;
                            }
                            if now > next_frame_time {
                                next_frame_time = now + frame_duration;
                            }
                        }
                        // Physical (device-pixel) window size drives the
                        // integer-scale + letterbox computation in draw().
                        let fb = gl_window.window().inner_size();
                        let (w, h) = (self.width as usize, self.height as usize);
                        let scaled = upscaler.apply(self.image(), w, h);
                        cx.draw(out_w as u32, out_h as u32, fb.width, fb.height, scaled);
                        gl_window.swap_buffers().unwrap();
                    } else {
                        // Unfocused: the emulator is paused, but a video
                        // consumer still expects frames at the wall-clock rate.
                        video.idle(std::time::Instant::now());
                    }

                    // Schedule the next draw. Late draws advance the deadline by
                    // whole periods for the same reason the frame deadline does
                    // — never reset it to `now` (see `docs/audio.md`) — but a
                    // hopeless backlog resyncs rather than drawing in a burst.
                    let now = std::time::Instant::now();
                    next_draw_time += draw_interval;
                    if now > next_draw_time {
                        next_draw_time = now + draw_interval;
                    }
                    *control_flow =
                        glutin::event_loop::ControlFlow::WaitUntil(next_draw_time);
                }
                _ => {
                    // Keep the current control flow
                }
            }
        });
    }

    #[cfg(all(feature = "tui", not(any(target_arch = "wasm32", feature = "ffi"))))]
    pub fn render_terminal(self) {
        // The terminal frontend now owns ROM loading (so it can offer a picker
        // and switch games in-process). A pre-built `Gameboy` carries no path,
        // so this legacy entry just opens the picker. Prefer `run_terminal`.
        // Fail loudly (audio/dead-code crew's intent) rather than swallowing.
        if let Err(e) = crate::screen::tui::run(None) {
            eprintln!("terminal frontend error: {e}");
            std::process::exit(1);
        }
    }

    #[inline]
    pub fn check_and_reset_gpu_updated(&mut self) -> bool {
        let result = self.cpu.memory.gpu.updated;
        self.cpu.memory.gpu.updated = false;
        result
    }

    /// Advance the machine by one frame — **or by one frame's worth of time**,
    /// whichever comes first.
    ///
    /// The second half of that is not a nicety. `Gpu::do_cycle` returns
    /// immediately while the LCD is off, so `updated` never fires and this loop
    /// has no other exit: a frame-paced frontend then runs the CPU without
    /// bound for as long as the game leaves the screen off. Measured on
    /// Pokémon Blue's boot, one call executed **845,836** cycles instead of
    /// 70,224 — twelve frames of emulation charged to one 16.743 ms slot, with
    /// the APU emitting the matching burst of samples that the audio queue's
    /// latency bound then has to throw away (`docs/audio.md` §4c).
    ///
    /// [`FRAME_TICKS`] is the real thing: 70,224 T-cycles at 4.194304 MHz is
    /// 59.7275 Hz. The cap carries a small margin because a single instruction
    /// can overshoot the boundary by a few cycles, so with the LCD **on** the
    /// cap is unreachable and this is byte-for-byte the old behaviour — the
    /// `updated` break always comes first. Only the LCD-off case changes, which
    /// is the whole bug.
    pub fn frame(&mut self) {
        self.begin_frame();

        let waitticks = (4194304f64 / 1000.0 * 16.0).round() as u32;
        let mut ticks = 0;
        let mut total = 0u32;

        'outer: loop {
            while ticks < waitticks {
                let step = self.cpu.do_cycle();
                ticks += step;
                total += step;
                if self.check_and_reset_gpu_updated() {
                    break 'outer;
                }
                if total >= FRAME_TICKS_CAP {
                    break 'outer;
                }
            }

            ticks -= waitticks;
        }

        self.end_frame();
    }

    /// The housekeeping [`Gameboy::frame`] does *before* the frame's cycles.
    ///
    /// Split out so a driver that owns the stepping loop — the two-console
    /// [`crate::link::LinkCable`], which has to interleave two cores instead of
    /// running one to its own frame boundary — still performs it, rather than
    /// silently skipping input, autosave and plugin dispatch.
    pub fn begin_frame(&mut self) {
        // Externally-scheduled button input is applied *before* the frame runs,
        // so a press is held for the whole of the frame the game samples it in.
        // One `Option` test when nothing is driving the game.
        self.cpu.memory.advance_input();
    }

    /// The housekeeping [`Gameboy::frame`] does *after* the frame's cycles.
    /// See [`Gameboy::begin_frame`].
    pub fn end_frame(&mut self) {
        // Battery-backed cartridge RAM is written out here, once the game has
        // left it alone for a moment — NOT in `Drop`, which the frontends
        // cannot guarantee runs (winit's event loop calls `process::exit`, the
        // release profile aborts on panic, and a closed window or `kill` skips
        // destructors entirely). See `crate::mbc::battery`.
        self.cpu.memory.mbc.autosave();

        // Publish the frame event. `on_frame` returns immediately when nothing
        // is attached and no trace is open, so this is one test per frame.
        let Gameboy { cpu, .. } = self;
        cpu.plugins.on_frame(&mut cpu.memory);
    }

    /// Write battery-backed cartridge RAM out now. Frontends should call this
    /// on a clean shutdown or before switching ROMs; `frame` already persists
    /// saves as they happen, so this only shortens the worst-case window.
    pub fn flush_battery(&mut self) {
        self.cpu.memory.mbc.flush_battery();
    }

    /// Why this machine is considered to have crashed, if it has — and with it,
    /// why the battery save is no longer being written. See
    /// [`MemoryManagementUnit::report_fault`](crate::mmu::MemoryManagementUnit::report_fault).
    /// A frontend should show this: the guard protects the save file, but only
    /// the user can act on it.
    pub fn fault(&self) -> Option<&str> {
        self.cpu.memory.fault()
    }

    /// Declare that the emulated program has left the rails, from outside the
    /// core — a frontend, a test, or a game-specific plugin that can recognise a
    /// crash the CPU-level detectors cannot. **Stops the battery save being
    /// written for the rest of the session**, and nothing else.
    pub fn report_fault(&mut self, reason: &str) {
        self.cpu.memory.report_fault(reason);
    }

    /// Borrow the running machine through the same capability-checked
    /// [`Machine`](crate::plugin::Machine) handle a plugin is given.
    ///
    /// This exists because the *perception* code in [`crate::agent::observe`]
    /// and the executor in [`crate::agent::action`] are written against
    /// `Machine`, not against `Gameboy` — deliberately, so that they cannot
    /// reach past what was granted. A host that wants to reuse them (the MCP
    /// server in [`crate::mcp`] is the first) would otherwise have to
    /// re-implement state reading against `Gameboy::peek`, which is exactly the
    /// duplication the plugin contract exists to prevent.
    ///
    /// The grant is the caller's own choice here, and that is honest rather
    /// than a hole: an embedder already owns the `Gameboy` and could call
    /// [`Gameboy::debug_write`] directly. Passing a narrow grant is how a host
    /// makes a *sub-component* — a tool handler, say — provably read-only.
    ///
    /// The handle borrows the MMU for the call and cannot be stashed.
    pub fn with_machine<R>(
        &mut self,
        granted: crate::plugin::Capability,
        f: impl FnOnce(&mut crate::plugin::Machine<'_>) -> R,
    ) -> R {
        let mut machine = crate::plugin::Machine::new(&mut self.cpu.memory, granted);
        f(&mut machine)
    }

    /// Attach a plugin. See [`crate::plugin`] for the contract; the frontend is
    /// responsible for telling the user one is active.
    pub fn add_plugin(
        &mut self,
        plugin: Box<dyn crate::plugin::Plugin>,
    ) -> Result<(), crate::plugin::RegisterError> {
        self.cpu.plugins.register(plugin)
    }

    /// Live status of every attached plugin, for the status bar.
    pub fn plugin_statuses(&self) -> Vec<crate::plugin::PluginStatus> {
        self.cpu.plugins.statuses()
    }

    /// Borrow an attached plugin by concrete type, so the frontend can render
    /// it. A plugin never draws — it exposes state and the frontend lays it out.
    pub fn plugin<T: crate::plugin::Plugin + 'static>(&self) -> Option<&T> {
        self.cpu.plugins.get::<T>()
    }

    /// Mutable counterpart of [`Gameboy::plugin`], for the frontend to
    /// configure the plugin it attached (whether its panel is on screen, say).
    pub fn plugin_mut<T: crate::plugin::Plugin + 'static>(&mut self) -> Option<&mut T> {
        self.cpu.plugins.get_mut::<T>()
    }

    /// One status-bar line describing attached plugins, or `None` when the
    /// machine is running unmodified.
    pub fn plugin_status_line(&self) -> Option<String> {
        self.cpu.plugins.status_line()
    }

    /// Open the `GB_PLUGIN_TRACE` NDJSON event log, if the environment asks for
    /// one. Returns a message to show the user on failure.
    pub fn open_plugin_trace(&mut self) -> Option<String> {
        self.cpu.plugins.open_trace_from_env()
    }

    pub fn flush_plugins(&mut self) {
        self.cpu.plugins.flush();
    }

    /// Serialise the complete machine state to bytes. The caller decides where
    /// the bytes live (a file, local storage, a download). See
    /// [`crate::savestate`] for the format.
    pub fn save_state(&self) -> Vec<u8> {
        self.cpu.save_state()
    }

    /// Restore machine state produced by [`Gameboy::save_state`]. Returns an
    /// error string on a corrupt, version-mismatched, or wrong-game state.
    pub fn load_state(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), crate::savestate::StateError> {
        let r = self.cpu.load_state(bytes);
        // Plugin state is NOT machine state and is not in the save file (see
        // docs/plugins.md). A link partner holding half a trade would desync
        // silently against a restored machine, so it is reset instead.
        if r.is_ok() {
            self.cpu.plugins.reset();
            // Queued input is external intent aimed at a machine that no longer
            // exists — the same argument as plugin state. Dropping it also
            // releases anything the queue was holding down.
            self.clear_input();
        }
        r
    }

    /// Cartridge title from the ROM header, for save-slot naming and stats.
    pub fn rom_title(&self) -> String {
        self.cpu.memory.mbc.romname()
    }

    /// Advance exactly one CPU instruction (`Cpu::do_cycle` executes one),
    /// returning the T-cycles it took. For the debug surface, fine-grained
    /// tests that need to land on instruction boundaries a whole-frame step
    /// would skip past, and the link cable — which pays out cycles to two cores
    /// alternately and needs to know how many it just spent.
    pub fn step_instruction(&mut self) -> u32 {
        self.cpu.do_cycle()
    }

    // ---------------------------------------------------------------------
    // The link port, as a cable sees it. Crate-private on purpose: the public
    // way to join two consoles is [`crate::link::LinkCable`], which owns the
    // pairing rules. Handing out the raw wire would let a caller deliver a
    // slave byte for a transfer that was never clocked.
    // ---------------------------------------------------------------------

    pub(crate) fn link_role(&self) -> crate::mmu::serial::LinkRole {
        self.cpu.memory.serial.link_role()
    }

    pub(crate) fn link_out(&self) -> u8 {
        self.cpu.memory.serial.link_out()
    }

    pub(crate) fn link_seq(&self) -> u32 {
        self.cpu.memory.serial.link_seq()
    }

    pub(crate) fn link_answer_master(&mut self, received: u8) {
        self.cpu.memory.serial.link_answer_master(received);
    }

    // Only `link::peer` (`#[cfg(unix)]`) drives the hold, so gate to match —
    // otherwise wasm and Windows builds warn on dead code.
    #[cfg(unix)]
    pub(crate) fn link_set_hold(&mut self, hold: bool) {
        self.cpu.memory.serial.link_set_hold(hold);
    }

    pub(crate) fn link_clock_slave(&mut self, received: u8) -> bool {
        self.cpu.memory.serial.link_clock_slave(received)
    }

    /// Whether the DMG/CGB HALT-bug latch is currently set — a briefly-live bit
    /// of CPU state. Exposed so tests can prove save states capture it.
    pub fn debug_halt_bug(&self) -> bool {
        self.cpu.halt_bug
    }

    /// Read a byte of emulated address space without perturbing timing — the
    /// Tier-1 inspector read of `docs/debugging.md`. Always compiled in; it
    /// costs nothing until called.
    pub fn peek(&mut self, address: u16) -> u8 {
        self.cpu.memory.debug_peek(address)
    }

    /// Read a run of bytes. See [`Gameboy::peek`].
    pub fn peek_range(&mut self, address: u16, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| self.cpu.memory.debug_peek(address.wrapping_add(i as u16)))
            .collect()
    }

    /// Program counter — cheap read for a debug/stats overlay.
    pub fn debug_pc(&self) -> u16 {
        self.cpu.registers.pc
    }

    /// Which of the two rendering engines the PPU is running — see
    /// [`crate::gpu::RenderMode`]. A frontend shows this so a player can tell
    /// which machine they are looking at; it is not a picture-quality setting.
    pub fn render_mode(&self) -> crate::gpu::RenderMode {
        self.cpu.memory.gpu.render_mode()
    }

    /// Switch rendering engine live. See [`crate::gpu::Gpu::set_render_mode`].
    pub fn set_render_mode(&mut self, mode: crate::gpu::RenderMode) {
        self.cpu.memory.gpu.set_render_mode(mode)
    }

    /// Declare whether this caller will read the framebuffer.
    ///
    /// `false` lets the PPU stop generating pixels — the largest saving
    /// available to a headless caller whose observation comes out of memory,
    /// and **guest-invisible by construction**. Pixels are on by default and
    /// nothing here makes them unavailable; see
    /// [`crate::gpu::Gpu::set_render_pixels`] for the whole argument, the one
    /// piece of state the skip still advances, and why the per-dot engine
    /// ignores it.
    ///
    /// With pixels off, [`Gameboy::image`] returns whatever was last drawn.
    pub fn set_render_pixels(&mut self, on: bool) {
        self.cpu.memory.gpu.set_render_pixels(on)
    }

    /// Whether pixels are being generated — see
    /// [`Gameboy::set_render_pixels`]. This is what was *asked for*;
    /// [`crate::gpu::Gpu::pixels_effective`] is what is actually happening.
    pub fn render_pixels(&self) -> bool {
        self.cpu.memory.gpu.render_pixels()
    }

    /// Read one byte through the memory bus, as the CPU would. For the debug
    /// surface and for tests that need to inspect cartridge RAM the way a game
    /// sees it — through the MBC's bank registers, not the raw save buffer.
    pub fn debug_read(&mut self, address: u16) -> u8 {
        self.cpu.memory.rb(address)
    }

    /// Write one byte through the memory bus, as the CPU would (so MBC control
    /// registers at 0x0000-0x7FFF behave exactly as for a real write).
    ///
    /// **Counted.** See [`Gameboy::host_writes`] — a rule like
    /// `no_memory_writes` is only worth anything if the host can prove it, and
    /// this is the one door a host has.
    pub fn debug_write(&mut self, address: u16, value: u8) {
        self.host_writes = self.host_writes.saturating_add(1);
        self.cpu.memory.wb(address, value)
    }

    /// How many bytes this session has written into the emulated machine from
    /// **outside the game** — through [`Gameboy::debug_write`].
    ///
    /// The evidence behind `src/scenario`'s `no_memory_writes` rule. A run that
    /// only pressed buttons reads zero here, and no amount of prompting can
    /// make a run that poked read zero.
    ///
    /// Session state, not machine state: it is not serialised, because it is a
    /// property of *this process's* conduct and a save state carries no claim
    /// about how it was produced. A recording carries that claim instead.
    pub fn host_writes(&self) -> u64 {
        self.host_writes
    }

    /// Currently-selected ROM bank — cheap read for a debug/stats overlay.
    pub fn debug_rom_bank(&self) -> usize {
        self.cpu.memory.mbc.rom_bank()
    }

    /// Cartridge header global checksum, for identifying a specific game build.
    pub fn global_checksum(&self) -> u16 {
        self.cpu.memory.mbc.global_checksum()
    }

    pub fn image(&self) -> &[u8] {
        &*self.cpu.memory.gpu.data
    }

    pub fn image_mut(&mut self) -> &mut [u8] {
        &mut *self.cpu.memory.gpu.data
    }

    pub fn keydown(&mut self, key: KeypadKey) {
        self.cpu.memory.keypad.keydown(key);
    }

    pub fn keyup(&mut self, key: KeypadKey) {
        self.cpu.memory.keypad.keyup(key);
    }

    /// Schedule a button press in **emulated frames** — hold `key` down for
    /// `hold` frames, then leave the pad idle for `gap`.
    ///
    /// This is the frontend's door onto the same queue a plugin reaches through
    /// [`crate::plugin::Machine::press`], and the only supported way to drive
    /// the game programmatically. See [`crate::input::InputQueue`].
    pub fn queue_press(&mut self, key: KeypadKey, hold: u16, gap: u16) {
        self.cpu.memory.input_queue.push(key, hold, gap);
    }

    /// Schedule a plain tap with the default hold and gap.
    pub fn tap(&mut self, key: KeypadKey) {
        self.cpu.memory.input_queue.tap(key);
    }

    /// Every scheduled press has been performed and released.
    pub fn input_idle(&self) -> bool {
        self.cpu.memory.input_queue.is_idle()
    }

    /// Abandon every scheduled press, releasing anything held.
    pub fn clear_input(&mut self) {
        let mem = &mut self.cpu.memory;
        mem.input_queue.reset(&mut mem.keypad);
    }

    /// True once any attached plugin has actually pressed a button — the
    /// frontend surfaces this so "the game is playing itself" is never a
    /// mystery.
    pub fn plugin_pressed_buttons(&self) -> bool {
        self.cpu.plugins.any_pressed_buttons()
    }

    /// Install a host audio sink, creating the APU if the machine has none.
    ///
    /// Available on **every** target, `wasm32` included — see
    /// [`crate::sound::web_audio`] for the browser sink and
    /// [`crate::sound::SilentPlayer`] for the one to install when there is no
    /// device, which is still a real sound chip.
    pub fn enable_audio(&mut self, player: Box<dyn crate::sound::AudioPlayer>) {
        self.cpu.memory.enable_audio(player);
    }

    /// Is an APU installed? False means the machine has **no sound chip**, not
    /// a quiet one — see `crate::sound::SilentPlayer` for why the difference
    /// is visible to the cartridge.
    pub fn audio_enabled(&self) -> bool {
        self.cpu.memory.sound.is_some()
    }

    /// Frontend hint: emulation is deliberately running faster than real time.
    /// The audio player then drops overflow instead of pacing the emulator
    /// against the audio clock (which would clamp fast-forward back to 1x).
    pub fn set_audio_fast_forward(&mut self, enabled: bool) {
        self.cpu.memory.set_audio_fast_forward(enabled);
    }
}

/// **The shiny hunter's driving seam.**
///
/// A plugin is a passenger: it is handed a borrowed [`crate::plugin::Machine`]
/// inside a callback and cannot press a button on a schedule, step a frame, load
/// a save state or start a second emulator. Hunting needs all four, so
/// `terminalgb-plugin-gen1-shinyhunt` states what it needs of a host as an
/// eight-method trait and this is the emulator's implementation of it. Every method
/// below is a one-line forward to an existing public `Gameboy` method — the
/// trait adds nothing the emulator did not already offer, except a way to reach
/// the attached hunter's latch, which the plugin API has no verb for.
///
/// It lives here rather than in the plugin crate because of the orphan rule and
/// the dependency direction: `gameboy` depends on its plugins, never the
/// reverse. `docs/shiny-hunt.md` §"What the plugin API could not do" records the
/// gap as a finding for the crew that owns the contract.
#[cfg(feature = "plugin-gen1-shinyhunt")]
impl terminalgb_plugin_gen1_shinyhunt::hunt::HuntMachine for Gameboy {
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        Gameboy::load_state(self, bytes).map_err(|e| e.to_string())
    }

    fn save_state(&self) -> Vec<u8> {
        Gameboy::save_state(self)
    }

    fn frame(&mut self) {
        Gameboy::frame(self)
    }

    fn key(&mut self, key: KeypadKey, down: bool) {
        if down {
            self.keydown(key)
        } else {
            self.keyup(key)
        }
    }

    fn peek(&mut self, address: u16) -> u8 {
        Gameboy::peek(self, address)
    }

    fn sighting(&self) -> Option<terminalgb_plugin_gen1_shinyhunt::Sighting> {
        self.plugin::<terminalgb_plugin_gen1_shinyhunt::ShinyHunt>()
            .and_then(|p| p.sighting())
    }

    fn arm(&mut self) {
        if let Some(p) = self.plugin_mut::<terminalgb_plugin_gen1_shinyhunt::ShinyHunt>()
        {
            p.arm();
        }
    }

    fn refusal(&self) -> Option<String> {
        match self.plugin::<terminalgb_plugin_gen1_shinyhunt::ShinyHunt>() {
            None => Some("no shiny-hunt plugin is attached to this machine".into()),
            Some(p) if p.game().is_none() => Some(format!(
                "`{}` is not a verified Pokémon Red/Blue cartridge — refusing to \
                 read another game's memory as if it were",
                p.detected_title().trim()
            )),
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mode::Model;

    /// A 32 KiB cartridge that turns the LCD **off** and then counts in a tight
    /// loop forever, writing the count to HRAM so a test can read how far it
    /// got. No commercial ROM, no vendored binary — see `testharness/battery.rs`
    /// for the same technique.
    ///
    /// One iteration is 52 T-cycles: `INC HL` 8, `LD A,H` 4, `LDH [n],A` 12,
    /// `LD A,L` 4, `LDH [n],A` 12, `JR` (taken) 12.
    const LOOP_TICKS: u32 = 52;

    fn lcd_off_spinner() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]); // NOP; JP $0150
        rom[0x134..0x13F].copy_from_slice(b"LCDOFFSPIN\0");
        rom[0x147] = 0x00; // ROM only
        rom[0x148] = 0x00; // 32 KiB
        rom[0x149] = 0x00; // no RAM
        let prog: &[u8] = &[
            0xAF, // XOR A            -> A = 0
            0xE0, 0x40, // LDH [$FF40], A  -> LCDC = 0, LCD off
            0x21, 0x00, 0x00, // LD HL, $0000
            // loop:
            0x23, // INC HL
            0x7C, // LD A, H
            0xE0, 0x80, // LDH [$FF80], A
            0x7D, // LD A, L
            0xE0, 0x81, // LDH [$FF81], A
            0x18, 0xF7, // JR loop  (-9)
        ];
        rom[0x150..0x150 + prog.len()].copy_from_slice(prog);
        rom
    }

    /// With the LCD off `Gpu::do_cycle` never sets `updated`, so `frame`'s only
    /// exit is the cycle cap. Without it this call **never returns at all** for
    /// this ROM, and returns after 845,836 cycles for a game that switches the
    /// screen back on later (`docs/audio.md` §4c) — twelve frames of emulation,
    /// and twelve frames of audio, charged to one 16.743 ms slot.
    ///
    /// Run on a worker thread with a deadline so a regression **fails** instead
    /// of hanging a CI job forever.
    #[test]
    fn a_frame_with_the_lcd_off_still_lasts_exactly_one_frame() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut gb = Gameboy::new(lcd_off_spinner(), None);
            let count = |gb: &mut Gameboy| {
                ((gb.peek(0xFF80) as u32) << 8) | gb.peek(0xFF81) as u32
            };
            // Switching the LCD off publishes one frame (the screen goes
            // blank), so the first call ends there. Every call *after* that has
            // no `updated` to break on and is the case under test.
            gb.frame();
            gb.frame();
            let before = count(&mut gb);
            gb.frame();
            let _ = tx.send((before, count(&mut gb)));
        });
        let (before, after) = rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("frame() never returned with the LCD off — the cap is gone");
        assert!(before > 0, "the test ROM never reached its loop");
        let iters = after - before;

        // The loop is entered a few cycles in, so allow a little slack either
        // side; what must not happen is *multiples* of a frame.
        let expected = FRAME_TICKS / LOOP_TICKS;
        assert!(
            iters > expected - 20 && iters < expected + 20,
            "ran {iters} iterations ({} cycles), expected about {expected} \
             ({FRAME_TICKS} cycles) — one frame's worth, not several",
            iters * LOOP_TICKS
        );
    }

    /// The grant handed to [`Gameboy::with_machine`] is **enforced**, not
    /// decorative. That is the whole reason a host reaches the machine this way
    /// instead of through `debug_read`/`debug_write`: it lets a host make one of
    /// its own sub-components — an MCP tool handler, a scripted driver —
    /// provably unable to write, rather than unable by convention. If this ever
    /// stops holding, `--allow-write-memory` becomes a comment.
    #[test]
    fn a_machine_handed_out_by_the_host_still_enforces_its_grant() {
        use crate::plugin::Capability;
        let mut gb = Gameboy::new(rom_only_cart(), None);
        let read_only = Capability::OBSERVE.union(Capability::READ_MEMORY);
        // WRAM is seeded with a fixed value so a run reproduces exactly — read
        // what is there rather than assuming zero, and write something that
        // differs from it, or "refused" and "landed" look identical.
        let seeded = gb.peek(0xC000);
        let other = seeded.wrapping_add(1);
        gb.with_machine(read_only, |m| {
            assert_eq!(m.peek(0xC000), Some(seeded), "reading was granted");
            assert!(!m.poke(0xC000, other), "writing was NOT granted");
            assert!(m.frame_rgba().is_some(), "observing was granted");
        });
        assert_eq!(
            gb.peek(0xC000),
            seeded,
            "the refused write must not have landed"
        );

        gb.with_machine(read_only.union(Capability::WRITE_MEMORY), |m| {
            assert!(m.poke(0xC000, other), "writing was granted this time");
        });
        assert_eq!(gb.peek(0xC000), other);

        // A grant of nothing reaches nothing at all.
        gb.with_machine(Capability::NONE, |m| {
            assert_eq!(m.peek(0xC000), None);
            assert!(m.frame_rgba().is_none());
            assert!(!m.set_button(crate::KeypadKey::A, true));
        });
    }

    /// A minimal ROM-only cartridge: header type `0x00`, 32 KiB, `NOP`s.
    fn rom_only_cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x00;
        rom
    }

    /// The banked cartridge-RAM read the box reader depends on: a numbered
    /// bank answers whether or not the game has RAM enabled or that bank
    /// mapped, a bank past the cartridge's storage answers `None` (absent
    /// storage must be distinguishable from a stored `$FF`), and a cartridge
    /// with no RAM at all answers `None` everywhere.
    #[test]
    fn sram_banks_are_readable_by_number_and_absence_is_not_a_value() {
        use crate::plugin::Capability;
        let read_only = Capability::OBSERVE.union(Capability::READ_MEMORY);

        // MBC3 + RAM + BATTERY-shaped header, 32 KiB RAM (4 banks) — what
        // Pokémon Red/Blue is. RAM starts zeroed and the game has it
        // *disabled*, which is exactly the state the reader meets.
        let mut rom = vec![0u8; 0x8000];
        rom[0x147] = 0x13;
        rom[0x149] = 0x03;
        let mut gb = Gameboy::new(rom, None);
        gb.with_machine(read_only, |m| {
            assert_eq!(m.peek_sram_bank(2, 0xA000), Some(0));
            assert_eq!(m.peek_sram_bank(3, 0xBFFF), Some(0));
            assert_eq!(m.peek_sram_bank(4, 0xA000), None, "no fifth bank");
            // The bus cannot see it: RAM is disabled, $A000 is open bus.
            assert_eq!(m.peek(0xA000), Some(0xFF));
        });

        let mut gb = Gameboy::new(rom_only_cart(), None);
        gb.with_machine(read_only, |m| {
            assert_eq!(m.peek_sram_bank(0, 0xA000), None, "no RAM at all");
        });
    }

    /// The embedding contract: a machine can be built, stepped and read with
    /// **no path at all**, so nothing on this path touches the filesystem.
    #[test]
    fn a_machine_runs_with_no_filesystem_and_no_environment() {
        let mut gb = Gameboy::new_with_model(rom_only_cart(), None, Model::Dmg);
        gb.keydown(KeypadKey::Start);
        gb.frame();
        gb.keyup(KeypadKey::Start);
        gb.frame();
        assert_eq!(gb.image().len(), 160 * 144 * 4);
        // Save states are in-memory bytes; the caller decides where they go.
        let s = gb.save_state();
        let mut other = Gameboy::new_with_model(rom_only_cart(), None, Model::Dmg);
        other
            .load_state(&s)
            .expect("state loads into a fresh machine");
    }

    /// Bytes from a user must not be able to abort the host process. Under
    /// `[profile.release] panic = 'abort'` there is no unwinding to catch.
    #[test]
    fn a_bad_cartridge_is_an_error_not_a_process_abort() {
        // Too short for a header.
        assert!(Gameboy::try_new_with_model(vec![0u8; 0x40], None, Model::Dmg).is_err());
        // A mapper we do not implement (0xFC = POCKET CAMERA).
        let mut cart = rom_only_cart();
        cart[0x147] = 0xFC;
        assert!(Gameboy::try_new_with_model(cart, None, Model::Dmg).is_err());
        // ...and a good one still works.
        assert!(Gameboy::try_new_with_model(rom_only_cart(), None, Model::Dmg).is_ok());
    }

    /// A cartridge that switches the LCD **on** over a scrolling background, so
    /// every frame draws a different picture and a missed scanline shows.
    ///
    /// `SCX` is advanced from the loop, which is what makes consecutive frames
    /// differ; the tile data is a diagonal so the shift is visible in the
    /// pixels rather than only in a counter.
    fn scrolling_picture_cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]); // NOP; JP $0150
        rom[0x134..0x13F].copy_from_slice(b"SCROLLPIC\0\0");
        rom[0x147] = 0x00; // ROM only
        let prog: &[u8] = &[
            // Tile 1: eight rows of alternating bits, written straight into VRAM.
            0x21, 0x10, 0x80, // LD HL, $8010     (tile 1)
            0x3E, 0x3C, // LD A, $3C
            0x06, 0x10, // LD B, 16
            // .fill:
            0x22, // LD [HL+], A
            0xEE, 0xFF, // XOR $FF   (so the two bitplanes differ)
            0x05, // DEC B
            0x20, 0xFA, // JR NZ, .fill
            // Tile map: every cell tile 1.
            0x21, 0x00, 0x98, // LD HL, $9800
            0x01, 0x00, 0x04, // LD BC, $0400
            0x3E, 0x01, // LD A, 1
            // .map:
            0x22, // LD [HL+], A
            0x0B, // DEC BC
            0x78, // LD A, B
            0xB1, // OR C
            0x3E, 0x01, // LD A, 1
            0x20, 0xF9, // JR NZ, .map
            // Palette, then LCD on with the background enabled.
            0x3E, 0xE4, // LD A, $E4
            0xE0, 0x47, // LDH [$FF47], A
            0x3E, 0x91, // LD A, $91
            0xE0, 0x40, // LDH [$FF40], A
            // .spin: scroll one pixel a pass, forever.
            0xF0, 0x43, // LDH A, [$FF43]
            0x3C, // INC A
            0xE0, 0x43, // LDH [$FF43], A
            0x18, 0xF9, // JR .spin
        ];
        rom[0x150..0x150 + prog.len()].copy_from_slice(prog);
        rom
    }

    fn frame_hash(gb: &Gameboy) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for b in gb.image() {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }

    /// **The guarantee action repetition rests on**: turn pixels back on at a
    /// frame boundary and the very next frame is byte-identical to one from a
    /// machine that never stopped drawing.
    ///
    /// That is what lets a caller pay for the picture only at the decision
    /// points — skip the intermediate frames, ask for the one the policy will
    /// actually look at. Without it, "skippable" would mean "and then you get a
    /// half-drawn frame", which is no use to a screen-only student.
    ///
    /// Teeth: make the skip path in `Gpu::renderscan` return without advancing
    /// the window line and this still passes (there is no window here) — the
    /// window case is covered by
    /// `gpu::tests::skipping_pixels_changes_nothing_the_cartridge_can_see`.
    /// Make `set_render_pixels` a no-op and the `assert_ne!` below fails
    /// instead, so neither direction can rot silently.
    #[test]
    fn pixels_turned_back_on_at_a_frame_boundary_give_an_exact_frame() {
        let mut drawing =
            Gameboy::new_with_model(scrolling_picture_cart(), None, Model::Dmg);
        let mut skipping =
            Gameboy::new_with_model(scrolling_picture_cart(), None, Model::Dmg);
        skipping.set_render_pixels(false);
        assert!(drawing.render_pixels() && !skipping.render_pixels());

        // Long enough for the ROM to have built its picture and be scrolling.
        for _ in 0..40 {
            drawing.frame();
            skipping.frame();
        }
        assert_ne!(
            frame_hash(&drawing),
            frame_hash(&skipping),
            "the skipping machine drew the same picture — the skip did nothing"
        );

        // The observation frame: declare the picture wanted, then step once.
        skipping.set_render_pixels(true);
        drawing.frame();
        skipping.frame();
        assert_eq!(
            frame_hash(&drawing),
            frame_hash(&skipping),
            "a frame drawn after re-enabling pixels is not the frame hardware would have drawn"
        );

        // And it is a real picture, not two identically blank ones.
        assert!(
            drawing
                .image()
                .chunks(4)
                .map(|p| p[1])
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 1,
            "the reference machine drew a uniform screen, so this test proved nothing"
        );
    }
}
