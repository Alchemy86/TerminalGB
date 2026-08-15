//! TerminalGB on a PlayStation Portable.
//!
//! The emulator core is used exactly as any other embedder uses it — `Gameboy`,
//! `frame`, `image`, `keydown`/`keyup` — with `--no-default-features`, so this
//! file is the *whole* PSP frontend. Nothing in `src/` knows a PSP exists.
//!
//! Three things here are not obvious and are the difference between 25 fps and
//! a locked 59.7275 Hz. They are written up in `docs/portability.md` §PSP.
//!
//!  1. **Ask for 333 MHz.** A PSP boots its CPU at 222 MHz. One call is worth
//!     1.5x and it is the single largest lever available.
//!  2. **Let the GE scale the picture.** 160x144 stretched to the panel by the
//!     MIPS core is minutes of CPU a frame; as a texture on one quad it is free.
//!     The CPU's only job is a row-wise copy into a power-of-two staging
//!     texture, in *cached* RAM — never a per-pixel write into uncached VRAM.
//!  3. **Report the WORK, not the paced frame rate.** The loop paces itself to
//!     the DMG's own 16.743 ms; a frame rate under a pacer says nothing about
//!     headroom, so the per-second line also carries mean and worst work time.
//!     Those are what say whether this machine can hold 59.7275 Hz.
#![feature(restricted_std)]
#![no_main]

use core::ffi::c_void;
use psp::sys::{
    self, ClearBuffer, DisplayPixelFormat, GuContextType, GuPrimitive, GuState, GuSyncBehavior,
    GuSyncMode, MipmapLevel, TextureColorComponent, TextureEffect, TextureFilter,
    TexturePixelFormat, VertexType,
};
use psp::vram_alloc::get_vram_allocator;
use psp::Align16;
use psp::{BUF_WIDTH, SCREEN_HEIGHT, SCREEN_WIDTH};
use std::path::PathBuf;

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;

psp::module!("terminalgb", 1, 0);

const GB_W: usize = 160;
const GB_H: usize = 144;
/// PSP textures are power-of-two on both axes, so 160x144 is staged inside a
/// 256x256 one and only that region is ever sampled.
const TEX: usize = 256;

/// The quad is always the WHOLE screen, and the aspect ratio is corrected
/// inside the texture instead: the picture is written at column [`PILLAR`] and
/// [`SAMPLED_W`] texels are sampled across the panel, so the untouched (black)
/// texels either side become the pillarbox for free.
///
/// Pillarboxing in the texture rather than by shrinking the quad costs nothing
/// — the copy has to choose a destination column anyway — and it keeps the
/// geometry to one case that is easy to check. Measured under PPSSPP at
/// 480x272: the picture lands on screen columns 89..391, which is
/// 160 x 272/144 = 302 px centred, to the pixel.
///
/// 160 x (272/144) = 302.2 px of a 480 px panel, so the sampled width is
/// 160 x 480 / 302.2.
const SAMPLED_W: f32 = GB_W as f32 * SCREEN_WIDTH as f32 * GB_H as f32
    / (SCREEN_HEIGHT as f32 * GB_W as f32);
const PILLAR: usize = ((SAMPLED_W as usize) - GB_W) / 2;

/// One emulated DMG frame: 70224 dots at 4.194304 MHz.
const FRAME_US: u64 = 16_743;

static mut LIST: Align16<[u32; 0x10000]> = Align16([0; 0x10000]);
/// Staging texture, in ordinary cached main memory. The GE reads it after a
/// `sceKernelDcacheWritebackRange`; writing it through the uncached VRAM
/// mirror instead costs ~1.5 ms a frame for no benefit.
static mut PIXELS: Align16<[u32; TEX * TEX]> = Align16([0; TEX * TEX]);

#[repr(C, align(4))]
#[derive(Clone, Copy)]
struct Vertex {
    u: f32,
    v: f32,
    x: f32,
    y: f32,
    z: f32,
}

static mut QUAD: Align16<[Vertex; 2]> = Align16([Vertex {
    u: 0.0,
    v: 0.0,
    x: 0.0,
    y: 0.0,
    z: 0.0,
}; 2]);

/// Where a memory stick keeps things. `host0:` is PPSSPP's or a devkit's view
/// of the developer's own disk, and is searched last so a real memory stick
/// always wins.
const ROM_DIRS: [&str; 4] = [
    "ms0:/PSP/GAME/terminalgb/roms",
    "ms0:/roms",
    "ms0:/PSP/GAME/terminalgb",
    "host0:/roms",
];

fn is_cartridge(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".gb") || lower.ends_with(".gbc")
}

fn find_cartridges() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for dir in ROM_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_cartridge)
            {
                found.push(path);
            }
        }
        if !found.is_empty() {
            break;
        }
    }
    found.sort();
    found
}

/// The pad must be configured before it is read. Without this
/// `sceCtrlReadBufferPositive` returns whatever the buffer happened to contain,
/// which showed up as a ROM picker scrolling by itself with nobody touching it.
fn init_pad() {
    unsafe {
        sys::sceCtrlSetSamplingCycle(0);
        sys::sceCtrlSetSamplingMode(sys::CtrlMode::Digital);
    }
}

/// **`Peek`, not `Read` — and the difference is a whole frame.**
///
/// `sceCtrlReadBufferPositive` *blocks* until the pad has been sampled again,
/// and with `sceCtrlSetSamplingCycle(0)` that sampling happens once per VBlank.
/// So a loop that calls it spends whatever is left of the display's 16.7 ms
/// inside it. That was invisible while the emulator took 23.9 ms a frame — the
/// sample was always already there, and the call returned at once. The moment
/// the core got fast enough to finish early it started waiting, and the wait
/// was charged to *emulation* in the per-frame statistics: the frame cost
/// **8.3 ms of work and reported 15.2**, and the loop was pinned to the panel's
/// 59.94 Hz rather than to the DMG's 59.7275.
///
/// `sceCtrlPeekBufferPositive` returns the latest sample without waiting, which
/// is what a self-paced loop wants: this one has its own deadline accumulator
/// and the emulated Game Boy has its own clock. Nothing else changes — the pad
/// is still sampled every VBlank, and the loop still reads it once a frame.
fn buttons() -> sys::CtrlButtons {
    let mut pad = sys::SceCtrlData::default();
    unsafe { sys::sceCtrlPeekBufferPositive(&mut pad, 1) };
    pad.buttons
}

/// The picker runs on the debug console, before the GE is initialised — the two
/// own the same framebuffer and must not both be live.
fn choose(roms: &[PathBuf]) -> PathBuf {
    if roms.is_empty() {
        psp::dprintln!("No .gb / .gbc found. Looked in:");
        for dir in ROM_DIRS {
            psp::dprintln!("  {}", dir);
        }
        psp::dprintln!("");
        psp::dprintln!("Copy a cartridge onto the memory stick and restart.");
        loop {
            unsafe { sys::sceDisplayWaitVblankStart() };
        }
    }
    if roms.len() == 1 {
        return roms[0].clone();
    }

    let draw = |sel: usize| {
        psp::dprintln!("");
        // Name the PSP BUTTON, not a letter. Under PPSSPP the keyboard `X`
        // key is CIRCLE and CROSS is on `Z`, so a prompt that says "X to
        // play" sends a tester's press to the wrong button and the build
        // reads as broken. See docs/portability.md §"Testing under PPSSPP".
        psp::dprintln!("Up/Down to choose. Press (X) CROSS, (O) CIRCLE or START");
        psp::dprintln!("to play  (PPSSPP keyboard: Z, X or Space).");
        for (i, rom) in roms.iter().enumerate() {
            let name = rom.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            psp::dprintln!("  {} {}", if i == sel { '>' } else { ' ' }, name);
        }
    };

    let mut sel = 0usize;
    draw(sel);
    let mut previous = buttons();
    loop {
        unsafe { sys::sceKernelDelayThread(40_000) };
        let now = buttons();
        let pressed = now & !previous;
        previous = now;

        // Forgiving on purpose: a menu has no reason to insist on one button,
        // and "nothing happened" is the worst thing a first launch can do.
        if pressed.intersects(
            sys::CtrlButtons::CROSS | sys::CtrlButtons::CIRCLE | sys::CtrlButtons::START,
        ) {
            return roms[sel].clone();
        }
        if pressed.contains(sys::CtrlButtons::DOWN) {
            sel = (sel + 1) % roms.len();
            draw(sel);
        } else if pressed.contains(sys::CtrlButtons::UP) {
            sel = (sel + roms.len() - 1) % roms.len();
            draw(sel);
        }
    }
}

struct Gpu {
    fbp0: *mut c_void,
}

impl Gpu {
    fn start() -> Self {
        let allocator = get_vram_allocator().unwrap();
        let fbp0 = allocator
            .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
            .unwrap()
            .as_mut_ptr_from_zero() as *mut c_void;
        let fbp1 = allocator
            .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
            .unwrap()
            .as_mut_ptr_from_zero() as *mut c_void;

        unsafe {
            sys::sceGuInit();
            sys::sceGuStart(GuContextType::Direct, &raw mut LIST as *mut c_void);
            sys::sceGuDrawBuffer(DisplayPixelFormat::Psm8888, fbp0, BUF_WIDTH as i32);
            sys::sceGuDispBuffer(
                SCREEN_WIDTH as i32,
                SCREEN_HEIGHT as i32,
                fbp1,
                BUF_WIDTH as i32,
            );
            sys::sceGuOffset(2048 - (SCREEN_WIDTH / 2), 2048 - (SCREEN_HEIGHT / 2));
            sys::sceGuViewport(2048, 2048, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
            sys::sceGuScissor(0, 0, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
            sys::sceGuEnable(GuState::ScissorTest);
            sys::sceGuEnable(GuState::Texture2D);
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            sys::sceDisplayWaitVblankStart();
            sys::sceGuDisplay(true);
        }
        Gpu { fbp0 }
    }

    /// Copy one emulated frame into the staging texture and draw it as a single
    /// scaled quad. Aspect is preserved: the DMG is 10:9 and the panel is
    /// 480x272, so the picture fills the height and is pillarboxed.
    ///
    /// Once this runs the GE owns the framebuffer and the debug console is
    /// gone, so the per-second statistics are written to `<rom>.stats.txt` on
    /// the memory stick instead of printed. A number nobody can read is a
    /// number nobody can check.
    fn present(&mut self, frame: &[u8], smooth: bool) {
        // Row-wise copy into the power-of-two texture, at PILLAR — the picture
        // is centred inside the *texture*, not inside the quad. `frame` is
        // already RGBA in the byte order the PSP's Psm8888 wants, so this is a
        // memcpy per scanline and not a per-pixel conversion.
        unsafe {
            let dst = (&raw mut PIXELS) as *mut u32;
            for y in 0..GB_H {
                core::ptr::copy_nonoverlapping(
                    frame.as_ptr().add(y * GB_W * 4) as *const u32,
                    dst.add(y * TEX + PILLAR),
                    GB_W,
                );
            }
            sys::sceKernelDcacheWritebackRange(dst as *const c_void, (TEX * GB_H * 4) as u32);
        }


        unsafe {
            let quad = (&raw mut QUAD) as *mut Vertex;
            quad.write(Vertex {
                u: 0.0,
                v: 0.0,
                x: 0.0,
                y: 0.0,
                z: 0.0,
            });
            quad.add(1).write(Vertex {
                u: SAMPLED_W,
                v: GB_H as f32,
                x: SCREEN_WIDTH as f32,
                y: SCREEN_HEIGHT as f32,
                z: 0.0,
            });

            sys::sceGuStart(GuContextType::Direct, &raw mut LIST as *mut c_void);
            sys::sceGuClearColor(0xff00_0000);
            sys::sceGuClear(ClearBuffer::COLOR_BUFFER_BIT);
            sys::sceGuTexMode(TexturePixelFormat::Psm8888, 0, 0, 0);
            sys::sceGuTexImage(
                MipmapLevel::None,
                TEX as i32,
                TEX as i32,
                TEX as i32,
                (&raw const PIXELS) as *const c_void,
            );
            sys::sceGuTexFunc(TextureEffect::Replace, TextureColorComponent::Rgb);
            // `TextureFilter` is not `Copy` in this binding, so build both.
            let (min, mag) = if smooth {
                (TextureFilter::Linear, TextureFilter::Linear)
            } else {
                (TextureFilter::Nearest, TextureFilter::Nearest)
            };
            sys::sceGuTexFilter(min, mag);
            sys::sceGuTexScale(1.0 / TEX as f32, 1.0 / TEX as f32);
            sys::sceGuTexOffset(0.0, 0.0);
            sys::sceGuDrawArray(
                GuPrimitive::Sprites,
                VertexType::TEXTURE_32BITF | VertexType::VERTEX_32BITF | VertexType::TRANSFORM_2D,
                2,
                core::ptr::null(),
                (&raw const QUAD) as *const c_void,
            );
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            self.fbp0 = sys::sceGuSwapBuffers();
        }
    }
}

/// A fixed-length benchmark that splits the frame budget by component.
///
/// **Measurement only** — `tools/psp-build.sh --profile`; never shipped. It
/// replaces the interactive loop because a benchmark that a human has to hold a
/// button through is a benchmark nobody re-runs.
///
/// The method, and why it is this one. The PSP's only clock costs **0.785 µs a
/// call** here and quantises to 1 µs, so timing individual calls is out: one
/// `bus_tick` is smaller than the measurement of it and there are ~70,000 a
/// frame. Whole *frames* are ~22,000 µs, where both problems vanish — so every
/// row below is a whole-frame time, and a component's cost is the difference
/// between a run that does its work and one that does not
/// (`terminalgb::profile`). Each configuration replays the **same save state**
/// from the same frame, and the report carries each run's executed-instruction
/// count so a reader can see whether two runs were comparable at all.
#[cfg(feature = "profile")]
fn bench(gb: &mut Gameboy, gpu: &mut Gpu, report_path: &std::path::Path) -> ! {
    use terminalgb::profile::{self, Switches};

    const WARM: u32 = 60;
    // Long enough to cover a whole phase of the game's own cycle. Pokémon
    // Blue's overworld alternates between a cheap frame and an expensive one and
    // has a slower period on top of that, so a 240-frame window lands wherever
    // it lands: measured 8.3 ms a frame over the 240 after a state load against
    // 15.2 over a run left going. Neither is wrong; only the long one is
    // comparable with what a player experiences.
    const RUN: u32 = 400;

    let base = gb.save_state();

    // FNV-1a over the frame, the same hash `examples/headless.rs` prints — it
    // is what says an optimisation changed the speed and not the picture.
    fn frame_hash(frame: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in frame {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    struct Row {
        name: &'static str,
        mean: f32,
        worst: f32,
        instructions: u32,
        halts: u32,
        batches: u32,
        reason: [u32; 6],
        bus_ticks: u32,
        scanlines: u32,
        hash: u64,
    }

    let mut rows: Vec<Row> = Vec::new();
    let mut present_mean = 0.0f32;

    let configs: [(&'static str, Switches, bool); 9] = [
        ("baseline", Switches::default(), false),
        (
            "no pixels",
            Switches {
                no_pixels: true,
                ..Switches::default()
            },
            false,
        ),
        (
            "no bus tick",
            Switches {
                no_bus_tick: true,
                ..Switches::default()
            },
            false,
        ),
        (
            "neither",
            Switches {
                no_pixels: true,
                no_bus_tick: true,
                ..Switches::default()
            },
            false,
        ),
        (
            "no idle batch",
            Switches {
                no_idle_batch: true,
                ..Switches::default()
            },
            false,
        ),
        (
            "no timer",
            Switches {
                no_timer: true,
                ..Switches::default()
            },
            false,
        ),
        (
            "no stat line",
            Switches {
                no_stat: true,
                ..Switches::default()
            },
            false,
        ),
        ("baseline + present", Switches::default(), true),
        // Last, because it changes the machine for good: the PSP frontend
        // installs no `AudioPlayer`, and with none the APU does not run at all
        // (`Sound::do_cycle` returns immediately) — so this row is what a sound
        // chip would cost if one were ever wired up.
        ("+ APU (no output)", Switches::default(), false),
    ];

    for (name, sw, with_present) in configs {
        if name.starts_with("+ APU") && !gb.audio_enabled() {
            gb.enable_audio(std::boxed::Box::new(terminalgb::sound::SilentPlayer));
        }
        let _ = gb.load_state(&base);
        profile::set_switches(sw);
        for _ in 0..WARM {
            gb.frame();
        }
        let _ = profile::take_counters();
        let mut total = 0u64;
        let mut worst = 0u64;
        for _ in 0..RUN {
            let t0 = now_us();
            gb.frame();
            if with_present {
                gpu.present(gb.image(), false);
            }
            let dt = now_us() - t0;
            total += dt;
            if dt > worst {
                worst = dt;
            }
        }
        let c = profile::take_counters();
        let mean = total as f32 / RUN as f32;
        if with_present {
            present_mean = mean - rows[0].mean;
        }
        rows.push(Row {
            name,
            mean,
            worst: worst as f32,
            instructions: c.instructions / RUN,
            halts: c.halts / RUN,
            batches: c.halt_batches / RUN,
            reason: c.idle_reason,
            bus_ticks: c.bus_ticks / RUN,
            scanlines: c.scanlines / RUN,
            hash: frame_hash(gb.image()),
        });
    }

    profile::set_switches(Switches::default());

    let mut out = std::string::String::new();
    out.push_str("TerminalGB PSP profile — PPSSPP, not real hardware\n");
    out.push_str(&std::format!("{} frames a row, warm {}\n\n", RUN, WARM));
    out.push_str(
        "row                 mean ms  worst ms   instr/f   halt/f  batch/f  bustick/f  lines/f  frame hash\n",
    );
    for r in &rows {
        out.push_str(&std::format!(
            "{:<18} {:8.2} {:9.2} {:9} {:8} {:8} {:10} {:8} {:#018x}\n",
            r.name,
            r.mean / 1000.0,
            r.worst / 1000.0,
            r.instructions,
            r.halts,
            r.batches,
            r.bus_ticks,
            r.scanlines,
            r.hash
        ));
    }
    out.push_str(&std::format!(
        "\nidle refusals (dma, serial, irq, no-ppu, timer, batched): {:?}\n",
        rows[0].reason
    ));
    let base_ms = rows[0].mean / 1000.0;
    let pixels = (rows[0].mean - rows[1].mean) / 1000.0;
    let bus = (rows[0].mean - rows[2].mean) / 1000.0;
    let neither = (rows[0].mean - rows[3].mean) / 1000.0;
    let apu = (rows[8].mean - rows[0].mean) / 1000.0;
    let unbatched = (rows[4].mean - rows[0].mean) / 1000.0;
    let timer = (rows[0].mean - rows[5].mean) / 1000.0;
    let stat = (rows[0].mean - rows[6].mean) / 1000.0;
    out.push_str(&std::format!(
        "\nemulation           {:8.2} ms\n\
         pixel rendering     {:8.2} ms  ({:.0}%)\n\
         per-access bus      {:8.2} ms  ({:.0}%)\n\
         both together       {:8.2} ms\n\
         everything else     {:8.2} ms  ({:.0}%)\n\
         idle batching saves {:8.2} ms\n\
         APU if installed    {:8.2} ms\n\
         timer (diagnostic)  {:8.2} ms  ({:.0}%)\n\
         STAT line (diag)    {:8.2} ms  ({:.0}%)\n\
         present (GE)        {:8.2} ms\n",
        base_ms,
        pixels,
        100.0 * pixels / base_ms,
        bus,
        100.0 * bus / base_ms,
        neither,
        base_ms - neither,
        100.0 * (base_ms - neither) / base_ms,
        unbatched,
        apu,
        timer,
        100.0 * timer / base_ms,
        stat,
        100.0 * stat / base_ms,
        present_mean / 1000.0,
    ));
    out.push_str(&std::format!(
        "\nbudget 16.743 ms — need {:.2}x\n",
        (base_ms + present_mean / 1000.0) / 16.743
    ));
    psp::dprintln!("{}", out);
    let _ = std::fs::write(report_path, out.as_bytes());
    unsafe { sys::sceKernelExitGame() };
    loop {
        unsafe { sys::sceDisplayWaitVblankStart() };
    }
}

fn now_us() -> u64 {
    unsafe { sys::sceKernelGetSystemTimeWide() as u64 }
}

fn psp_main() {
    psp::enable_home_button();
    // Lever 1, and the biggest single one: a PSP boots at 222 MHz.
    unsafe { sys::scePowerSetClockFrequency(333, 333, 166) };

    init_pad();
    psp::dprintln!("TerminalGB");
    let roms = find_cartridges();
    let path = choose(&roms);
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string();
    psp::dprintln!("loading {}", name);

    let rom = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            psp::dprintln!("cannot read {}: {}", name, e);
            loop {
                unsafe { sys::sceDisplayWaitVblankStart() };
            }
        }
    };

    // `Some(path)` is what wires the battery up: cartridge RAM is written back
    // to `<rom>.sav` beside the cartridge, on the memory stick. There is no
    // `$HOME` on a PSP and the core never looks for one.
    let mut gb = match Gameboy::try_new_with_model(rom, Some(path.clone()), terminalgb::mode::Model::Auto) {
        Ok(gb) => gb,
        Err(e) => {
            psp::dprintln!("{}: {}", name, e);
            loop {
                unsafe { sys::sceDisplayWaitVblankStart() };
            }
        }
    };
    psp::dprintln!("{} ({})", gb.rom_title(), name);

    // A save state beside the cartridge is loaded at boot. That is how a
    // handheld with no keyboard gets dropped into the middle of a game, and it
    // is what makes a benchmark reproducible: the overworld and a battle are
    // very different loads and neither is reachable by mashing buttons.
    let state_path = path.with_extension("gbss");
    let stats_path = path.with_extension("stats.txt");
    if let Ok(bytes) = std::fs::read(&state_path) {
        match gb.load_state(&bytes) {
            Ok(()) => psp::dprintln!("loaded state ({} bytes)", bytes.len()),
            Err(e) => psp::dprintln!("state ignored: {:?}", e),
        }
    }

    let mut gpu = Gpu::start();

    #[cfg(feature = "profile")]
    bench(&mut gb, &mut gpu, &path.with_extension("prof.txt"));

    let keys = [
        (sys::CtrlButtons::UP, KeypadKey::Up),
        (sys::CtrlButtons::DOWN, KeypadKey::Down),
        (sys::CtrlButtons::LEFT, KeypadKey::Left),
        (sys::CtrlButtons::RIGHT, KeypadKey::Right),
        (sys::CtrlButtons::CROSS, KeypadKey::A),
        (sys::CtrlButtons::CIRCLE, KeypadKey::B),
        (sys::CtrlButtons::START, KeypadKey::Start),
        (sys::CtrlButtons::SELECT, KeypadKey::Select),
    ];

    let mut smooth = false;
    let mut previous = sys::CtrlButtons::empty();
    let mut frames: u64 = 0;
    let mut worst_us: u64 = 0;
    let mut work_us: u64 = 0;
    let mut emulate_us: u64 = 0;
    let mut window_frames: u64 = 0;
    let mut over_budget: u64 = 0;
    let mut stats = std::string::String::new();
    let mut window_start = now_us();
    let mut deadline = now_us();

    loop {
        let began = now_us();

        let now = buttons();
        let pressed = now & !previous;
        previous = now;
        for (bit, key) in keys {
            if now.contains(bit) {
                gb.keydown(key);
            } else {
                gb.keyup(key);
            }
        }
        if pressed.contains(sys::CtrlButtons::TRIANGLE) {
            smooth = !smooth;
        }
        if pressed.contains(sys::CtrlButtons::LTRIGGER) {
            let bytes = gb.save_state();
            let _ = std::fs::write(&state_path, &bytes);
        }
        if pressed.contains(sys::CtrlButtons::RTRIGGER) {
            if let Ok(bytes) = std::fs::read(&state_path) {
                let _ = gb.load_state(&bytes);
            }
        }

        gb.frame();
        let emulated_at = now_us();
        gpu.present(gb.image(), smooth);

        frames += 1;
        window_frames += 1;
        // `spent` is emulate + present, measured BEFORE the pacer sleeps: the
        // work, not the wall clock. A paced 59.73 fps says nothing about
        // headroom on its own — this is what says whether there is any.
        let spent = now_us() - began;
        work_us += spent;
        emulate_us += emulated_at - began;
        if spent > worst_us {
            worst_us = spent;
        }
        if spent > FRAME_US {
            over_budget += 1;
        }

        // A deadline accumulator, never a reset to "now" — the same rule the
        // desktop and terminal frontends follow. If the machine cannot keep up
        // the emulator runs flat out rather than accumulating an unpayable
        // debt.
        deadline += FRAME_US;
        let now = now_us();
        if now < deadline {
            unsafe { sys::sceKernelDelayThread((deadline - now) as u32) };
        } else if now - deadline > FRAME_US * 8 {
            deadline = now;
        }

        // The window ends *here*, and the next one starts on the same reading —
        // no gap. Taking a fresh `now_us()` after writing the report instead
        // left the report's own I/O outside both windows, and a 32 KiB battery
        // flush to a memory stick is not free: the frames it delayed were then
        // counted against a window that had not paid for them, and the loop
        // reported **61 fps against a 59.7275 Hz** pacer it was in fact keeping.
        let window_end = now_us();
        let elapsed = window_end - window_start;
        if elapsed >= 2_000_000 {
            let line = std::format!(
                "{} fps {:.2} | work {:.1} (emu {:.1} + present {:.1}) ms | worst {:.1} ms | over {}/{} | f {}",
                gb.rom_title(),
                window_frames as f32 * 1_000_000.0 / elapsed as f32,
                work_us as f32 / window_frames as f32 / 1000.0,
                emulate_us as f32 / window_frames as f32 / 1000.0,
                (work_us - emulate_us) as f32 / window_frames as f32 / 1000.0,
                worst_us as f32 / 1000.0,
                over_budget,
                window_frames,
                frames
            );
            psp::dprintln!("{}", line);
            // Also to the memory stick. Once the GE owns the framebuffer the
            // debug console is invisible, and a number nobody can read is a
            // number nobody can check — this is what a benchmark run is read
            // back from. **Appended**, not overwritten: a run's *last* window
            // is whatever the game happened to be doing when the emulator was
            // stopped, which is not a measurement of anything. The whole series
            // is, and it is reproducible — a save state, no input and
            // deterministic emulation replay the same frames every time.
            // No `flush_battery()` here. `Gameboy::frame` already writes
            // cartridge RAM out once the game has left it alone for a moment,
            // so forcing a second write every two seconds bought no durability
            // and cost a **59 ms frame** on the memory stick — invisible while
            // the emulator was 1.4x too slow to have a frame budget to blow.
            stats.push_str(&line);
            stats.push('\n');
            let _ = std::fs::write(&stats_path, stats.as_bytes());
            window_frames = 0;
            over_budget = 0;
            worst_us = 0;
            work_us = 0;
            emulate_us = 0;
            window_start = window_end;
        }
    }
}
