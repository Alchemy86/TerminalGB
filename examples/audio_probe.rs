//! Audio device probe — reports what output configuration this machine's cpal
//! backend actually offers and what `CpalPlayer` negotiates from it. Run it on
//! any box to confirm the real sample rate and channel count before trusting
//! your ears:
//!
//! ```text
//! cargo run --features audio --example audio_probe
//! ```
//!
//! The negotiated rate it prints is exactly the rate the APU is told to resample
//! to (`Sound::set_player` retunes blip_buf to it), so if this rate matches the
//! rate the stream truly runs at, pitch is correct by construction. See
//! `docs/audio.md` for the full reasoning.
#[cfg(feature = "audio")]
fn main() {
    use cpal::traits::{DeviceTrait, HostTrait};
    use terminalgb::sound::AudioPlayer;

    let host = cpal::default_host();
    match host.default_output_device() {
        Some(dev) => {
            println!("default device: {:?}", dev.name());
            match dev.default_output_config() {
                Ok(c) => println!("device DEFAULT config: {c:?}"),
                Err(e) => println!("default_output_config err: {e}"),
            }
            println!("--- supported output configs (enumeration order) ---");
            if let Ok(cfgs) = dev.supported_output_configs() {
                for (i, c) in cfgs.enumerate() {
                    println!(
                        "[{i}] fmt={:?} ch={} min={} max={}",
                        c.sample_format(),
                        c.channels(),
                        c.min_sample_rate().0,
                        c.max_sample_rate().0
                    );
                }
            }
        }
        None => println!("NO default output device"),
    }

    match terminalgb::sound::cpal_audio::CpalPlayer::new() {
        Some((p, _stream)) => {
            println!(
                ">>> CpalPlayer negotiated sample_rate = {}",
                p.samples_rate()
            )
        }
        None => println!(">>> CpalPlayer::new() returned None (no usable output)"),
    }
}

#[cfg(not(feature = "audio"))]
fn main() {
    println!("audio probe needs the `audio` feature: cargo run --features audio --example audio_probe");
}
