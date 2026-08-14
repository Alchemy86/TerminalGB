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
            eprintln!("gameboy: {w}");
        }
        // This frontend has no key to turn the stream on, so an armed-but-idle
        // stream would be unreachable. Say what to add rather than leaving a
        // configured destination looking like a setting that was ignored.
        let video_spec = match video_spec {
            Some(s) if !video_start => {
                eprintln!(
                    "gameboy: video: {} configured but not streaming — add \
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
        }
        r
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
