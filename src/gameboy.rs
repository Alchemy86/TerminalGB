use crate::cpu::core::Cpu;
use crate::input::KeypadKey;

pub struct Gameboy {
    cpu: Cpu<'static>,
    pub width: u32,
    pub height: u32,
}

pub use self::Target::{GameBoy, GameBoyColor, SuperGameBoy};

#[derive(PartialEq, Eq, Debug, Copy, Clone, Default)]
pub enum RenderMode {
    #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
    #[default]
    Desktop,
    #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
    Terminal,
    #[cfg(any(target_arch = "wasm32", feature = "ffi"))]
    #[default]
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

/// Launch the terminal frontend. `initial_rom` starts straight into that game;
/// `None` opens the ROM picker. This is the entry point terminal binaries
/// should use — it lets the TUI own ROM loading, so the picker and in-app ROM
/// switching work.
#[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
pub fn run_terminal(initial_rom: Option<std::path::PathBuf>) {
    // Fail loudly (and non-zero) on setup errors — e.g. a terminal with no
    // kitty keyboard support — rather than silently returning.
    if let Err(e) = crate::screen::tui::run(initial_rom) {
        eprintln!("terminal frontend error: {e}");
        std::process::exit(1);
    }
}

impl Gameboy {
    pub fn new(data: Vec<u8>, filepath: Option<std::path::PathBuf>) -> Gameboy {
        // let rom = load_rom();

        let gb = Gameboy {
            cpu: Cpu::new(data, filepath),
            width: 160,
            height: 144,
        };

        gb
    }

    pub fn render(self, render_mode: RenderMode) {
        match render_mode {
            #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
            RenderMode::Desktop => {
                self.render_desktop();
            }
            #[cfg(any(target_arch = "wasm32", feature = "ffi"))]
            RenderMode::WebAssembly => {
                // crate::screen::web::render();
            }
            #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
            RenderMode::Terminal => {
                self.render_terminal();
            }
        }
    }

    #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
    pub fn render_desktop(mut self) {
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

        // Pixel-art upscaler (see crate::upscale): a CPU pre-pass on the
        // 160x144 frame before the texture upload. Chosen over a GL shader
        // because the same code then serves every frontend (terminal, wasm)
        // and the measured cost (<2 ms worst case, MMPX x4) is far inside the
        // 16.7 ms frame budget. Initial algorithm/factor come from the shared
        // config file; U cycles the algorithm live, I cycles the factor.
        let cfg = crate::screen::config::Config::load();
        let mut upscaler = crate::upscale::Upscaler::new(cfg.upscale, cfg.upscale_factor);
        let (mut out_w, mut out_h) =
            upscaler.output_size(self.width as usize, self.height as usize);

        // Raw video-out (see `crate::video`). Off unless configured. Unlike the
        // terminal frontend this one may stream to **stdout**, because nothing
        // else here writes there — so the documented pipe works verbatim:
        //   desktop game.gb --video-out - | ffmpeg -f rawvideo -pix_fmt rgba \
        //       -s 160x144 -framerate 59.7275 -i - out.mp4
        let (video_spec, video_warnings) = crate::video::VideoSpec::resolve(&cfg.video);
        for w in &video_warnings {
            eprintln!("gameboy: {w}");
        }
        let mut video = match video_spec {
            Some(mut s) => {
                // Fix the geometry from the configured upscaler so the size the
                // consumer is told is the truth from the first byte.
                if s.source == crate::video::Source::Display && s.size.is_none() {
                    s.size = Some((out_w as u32, out_h as u32));
                }
                let (v, err) = crate::video::VideoOut::open(s);
                if let Some(e) = err {
                    eprintln!("gameboy: {e}");
                }
                if v.is_enabled() {
                    eprintln!("gameboy: {}", v.describe());
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
        let inner_size = glutin::dpi::LogicalSize {
            width: out_w as u32,
            height: out_h as u32,
        };
        let window_builder = glutin::window::WindowBuilder::new()
            .with_title("Gameboy")
            .with_inner_size(inner_size)
            .with_resizable(true);
        let gl_window = glutin::ContextBuilder::new()
            .build_windowed(window_builder, &event_loop)
            .unwrap();
        let gl_window = unsafe { gl_window.make_current().unwrap() };

        gl::load_with(|s| gl_window.get_proc_address(s) as *const _);

        let cx = Glcx::new();
        let mut focused = true;
        // The DMG refreshes every 70224 CPU cycles at 4.194304 MHz = 59.7275 Hz,
        // i.e. one frame every 16.7427 ms. The old value (16 ms = 62.5 fps) paced
        // the whole emulator ~4.64% fast, which the player hears as the game (and
        // its music) running slightly fast AND as audio roughness: the APU then
        // produces ~4.6% more samples per wall-second than the sound card drains,
        // so the output queue overflows and drops samples periodically. Pacing at
        // the true frame period fixes both — see docs/audio.md.
        let frame_duration = std::time::Duration::from_micros(16_743); // 59.73 FPS (true DMG)

        // Deadline accumulator, NOT `last = Instant::now()` after each frame.
        // Resetting to `now` adds the draw/scheduling overhead to every period,
        // so the emulator runs a few percent slow on average — and audio sample
        // production is slaved to emulation speed, so a slow producer drains
        // the audio queue and underruns ("blocky" sound). Advancing the
        // deadline by exactly one period keeps the long-run average at
        // 59.7275 Hz; a bounded catch-up absorbs late wakeups.
        let mut next_frame_time = std::time::Instant::now() + frame_duration;

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
                            let changed = match input.virtual_keycode {
                                Some(glutin::event::VirtualKeyCode::U) => {
                                    upscaler.cycle_algorithm();
                                    true
                                }
                                Some(glutin::event::VirtualKeyCode::I) => {
                                    upscaler.cycle_factor();
                                    true
                                }
                                _ => false,
                            };
                            if changed {
                                (out_w, out_h) = upscaler.output_size(
                                    self.width as usize,
                                    self.height as usize,
                                );
                                window.set_title(&format!(
                                    "Gameboy — {}",
                                    upscaler.label()
                                ));
                                window.request_redraw();
                            }
                        }
                    }
                    *control_flow =
                        process_window(window, &wevent, &mut self, &mut focused)
                }
                glutin::event::Event::MainEventsCleared => {
                    // Only request redraw if the frame deadline has passed
                    if std::time::Instant::now() >= next_frame_time {
                        window.request_redraw();
                    }
                }
                glutin::event::Event::RedrawRequested(_) => {
                    if focused {
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
                            while now > next_frame_time && extra < 3 {
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

                    // Schedule next frame
                    *control_flow =
                        glutin::event_loop::ControlFlow::WaitUntil(next_frame_time);
                }
                _ => {
                    // Keep the current control flow
                }
            }
        });
    }

    #[cfg(not(any(target_arch = "wasm32", feature = "ffi")))]
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

    pub fn frame(&mut self) {
        let waitticks = (4194304f64 / 1000.0 * 16.0).round() as u32;
        let mut ticks = 0;

        'outer: loop {
            while ticks < waitticks {
                ticks += self.cpu.do_cycle();
                if self.check_and_reset_gpu_updated() {
                    break 'outer;
                }
            }

            ticks -= waitticks;
        }

        // Battery-backed cartridge RAM is written out here, once the game has
        // left it alone for a moment — NOT in `Drop`, which the frontends
        // cannot guarantee runs (winit's event loop calls `process::exit`, the
        // release profile aborts on panic, and a closed window or `kill` skips
        // destructors entirely). See `crate::mbc::battery`.
        self.cpu.memory.mbc.autosave();
    }

    /// Write battery-backed cartridge RAM out now. Frontends should call this
    /// on a clean shutdown or before switching ROMs; `frame` already persists
    /// saves as they happen, so this only shortens the worst-case window.
    pub fn flush_battery(&mut self) {
        self.cpu.memory.mbc.flush_battery();
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
        self.cpu.load_state(bytes)
    }

    /// Cartridge title from the ROM header, for save-slot naming and stats.
    pub fn rom_title(&self) -> String {
        self.cpu.memory.mbc.romname()
    }

    /// Advance exactly one CPU instruction (`Cpu::do_cycle` executes one). For
    /// the debug surface and fine-grained tests that need to land on
    /// instruction boundaries a whole-frame step would skip past.
    pub fn step_instruction(&mut self) {
        self.cpu.do_cycle();
    }

    /// Whether the DMG/CGB HALT-bug latch is currently set — a briefly-live bit
    /// of CPU state. Exposed so tests can prove save states capture it.
    pub fn debug_halt_bug(&self) -> bool {
        self.cpu.halt_bug
    }

    /// Program counter — cheap read for a debug/stats overlay.
    pub fn debug_pc(&self) -> u16 {
        self.cpu.registers.pc
    }

    /// Read one byte through the memory bus, as the CPU would. For the debug
    /// surface and for tests that need to inspect cartridge RAM the way a game
    /// sees it — through the MBC's bank registers, not the raw save buffer.
    pub fn debug_read(&mut self, address: u16) -> u8 {
        self.cpu.memory.rb(address)
    }

    /// Write one byte through the memory bus, as the CPU would (so MBC control
    /// registers at 0x0000-0x7FFF behave exactly as for a real write).
    pub fn debug_write(&mut self, address: u16, value: u8) {
        self.cpu.memory.wb(address, value)
    }

    /// Currently-selected ROM bank — cheap read for a debug/stats overlay.
    pub fn debug_rom_bank(&self) -> usize {
        self.cpu.memory.mbc.rom_bank()
    }

    /// Read-only snapshot access for plugins and debuggers — Tier 1 of
    /// [`docs/debugging.md`](../docs/debugging.md). Pure reads: safe to call
    /// between steps or frames, and free when nobody calls it.
    pub fn debug(&self) -> crate::plugin::DebugView<'_> {
        crate::plugin::DebugView::new(self)
    }

    /// Side-effect-free bus read backing [`Gameboy::debug`].
    pub fn peek(&self, addr: u16) -> u8 {
        self.cpu.memory.peek(addr)
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

    #[cfg(not(target_arch = "wasm32"))]
    pub fn enable_audio(&mut self, player: Box<dyn crate::sound::AudioPlayer>) {
        self.cpu.memory.enable_audio(player);
    }

    /// Frontend hint: emulation is deliberately running faster than real time.
    /// The audio player then drops overflow instead of pacing the emulator
    /// against the audio clock (which would clamp fast-forward back to 1x).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn set_audio_fast_forward(&mut self, enabled: bool) {
        self.cpu.memory.set_audio_fast_forward(enabled);
    }
}
