//! Play a tone through the **real sound card** at each rung of the playback
//! volume ladder, so the control can be judged the only way an audible feature
//! can be: by listening to it, or by recording what comes out.
//!
//! ```text
//! cargo run --release --features audio --example audio_volume            # ladder
//! cargo run --release --features audio --example audio_volume -- 30      # one level
//! cargo run --release --features audio --example audio_volume -- mute
//! ```
//!
//! Each rung is announced on stdout as it starts, so a recording of the
//! system's monitor source can be sliced against the log (that is how the level
//! ladder was measured — see `docs/audio.md` §"Playback volume"):
//!
//! ```text
//! parec --device <sink>.monitor --format=s16le --rate=44100 --channels=2 > out.raw &
//! cargo run --release --features audio --example audio_volume
//! ```
//!
//! **The emulated machine is identical at every rung.** Volume is applied in
//! the cpal callback, to finished samples — never to NR50, which is emulated
//! behaviour the accuracy suites measure. See `gameboy::sound::output_volume`.
#[cfg(feature = "audio")]
fn main() {
    use gameboy::sound::output_volume;
    use std::time::{Duration, Instant};

    // How long each rung plays for.
    const DWELL: Duration = Duration::from_millis(1500);

    let arg = std::env::args().nth(1);
    let ladder: Vec<(String, u8, bool)> = match arg.as_deref() {
        None => (0..=10)
            .rev()
            .map(|n| n * 10)
            .map(|l: u8| (format!("{l}%"), l, false))
            .chain(std::iter::once(("mute".to_string(), 50, true)))
            .collect(),
        Some("mute") | Some("muted") => vec![("mute".to_string(), 50, true)],
        Some(v) => match v.trim_end_matches('%').parse::<u8>() {
            Ok(l) if l <= output_volume::MAX => vec![(format!("{l}%"), l, false)],
            _ => {
                eprintln!("usage: audio_volume [0-100 | mute]");
                std::process::exit(2);
            }
        },
    };

    let (cpu, _rom_path) = tone_machine();
    let mut cpu = cpu;

    let (player, _stream) = match gameboy::sound::cpal_audio::CpalPlayer::new() {
        Some(x) => x,
        None => {
            eprintln!(
                "no audio device: {}",
                gameboy::sound::host_status::snapshot().label()
            );
            std::process::exit(1);
        }
    };
    let rate = {
        use gameboy::sound::AudioPlayer;
        player.samples_rate()
    };
    cpu.memory.enable_audio(Box::new(player));

    println!(
        "device rate {rate} Hz — {} rung(s), {:?} each",
        ladder.len(),
        DWELL
    );
    println!("the emulated machine is IDENTICAL at every rung; only playback changes");

    const FRAME: Duration = Duration::from_micros(16_743);
    let mut next_frame = Instant::now() + FRAME;
    for (label, level, muted) in ladder {
        output_volume::set_level(level);
        output_volume::set_muted(muted);
        println!(
            "  {label:>6}  gain {:.4}  ({})",
            output_volume::gain(),
            output_volume::label()
        );
        let until = Instant::now() + DWELL;
        while Instant::now() < until {
            loop {
                cpu.do_cycle();
                if cpu.memory.gpu.updated {
                    cpu.memory.gpu.updated = false;
                    break;
                }
            }
            let now = Instant::now();
            if now < next_frame {
                std::thread::sleep(next_frame - now);
            } else if now > next_frame + 4 * FRAME {
                next_frame = now;
            }
            next_frame += FRAME;
        }
    }
    output_volume::set_muted(false);
    println!("done");
}

/// A machine whose only job is to hold a 440 Hz square wave on channel 1 at
/// full envelope volume — the loudest thing the APU can be asked for, so the
/// top of the ladder is a fair test for clipping.
#[cfg(feature = "audio")]
fn tone_machine() -> (gameboy::cpu::core::Cpu<'static>, std::path::PathBuf) {
    use gameboy::cpu::core::Cpu;

    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x134..0x134 + 8].copy_from_slice(b"TONEVOLU");
    let mut chk: u8 = 0;
    for &b in rom.iter().take(0x14D).skip(0x134) {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x14D] = chk;
    let prog: &[u8] = &[
        0x3E, 0x80, 0xE0, 0x26, // NR52 on
        0x3E, 0xFF, 0xE0, 0x25, // NR51 all channels both sides
        0x3E, 0x77, 0xE0, 0x24, // NR50 max — the emulated master volume,
        //                          left exactly where a game would put it
        0x3E, 0x80, 0xE0, 0x11, // NR11 duty 50%
        0x3E, 0xF0, 0xE0, 0x12, // NR12 volume 15, DAC on
        0x3E, 0xD6, 0xE0, 0x13, // NR13: 1750 & 0xFF  -> 440 Hz
        0x3E, 0x86, 0xE0, 0x14, // NR14: trigger | (1750 >> 8)
        0x18, 0xFE, // spin
    ];
    rom[0x150..0x150 + prog.len()].copy_from_slice(prog);

    let dir = std::path::PathBuf::from("target/testrom-artifacts/audio");
    std::fs::create_dir_all(&dir).unwrap();
    let rom_path = dir.join("tonevolume.gb");
    std::fs::write(&rom_path, &rom).unwrap();
    let cpu = Cpu::new(rom, Some(rom_path.clone()));
    (cpu, rom_path)
}

#[cfg(not(feature = "audio"))]
fn main() {
    println!(
        "audio_volume needs the `audio` feature: \
         cargo run --release --features audio --example audio_volume"
    );
}
