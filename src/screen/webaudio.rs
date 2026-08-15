//! Web Audio playback for the browser frontend — the twenty lines that
//! actually touch an `AudioContext`.
//!
//! Everything that can be reasoned about without a browser lives in
//! [`crate::sound::web_audio`]: the producer half (a plain
//! [`sample_queue::SampleQueue`](crate::sound::sample_queue::SampleQueue), so
//! nothing that is not `Send` goes near the APU) and the scheduling decisions
//! (so the anti-glitch rules are unit-tested under `cargo test` on the host).
//! This file is the `web-sys` shell around them, and lives in
//! [`crate::screen`] because that is where host assumptions belong.
//!
//! ## The two browser rules that are not obvious
//!
//! 1. **An `AudioContext` starts `suspended` unless it is created during a
//!    user gesture**, and stays suspended until `resume()` is called from one
//!    ([Chrome's autoplay policy](https://developer.chrome.com/blog/autoplay/#webaudio),
//!    [the spec's "allowed to start" rule](https://www.w3.org/TR/webaudio/#allowed-to-start)).
//!    `render()` is called from the page's Play button handler, so the context
//!    is created inside a gesture; [`WebAudio::nudge`] re-`resume()`s on every
//!    key press anyway, because a context can also be suspended later (a
//!    backgrounded tab) and a silent emulator with no explanation is the worst
//!    possible outcome.
//! 2. **`ctx.sampleRate` is the device's, not ours to choose.** 48 kHz is as
//!    common as 44.1 kHz. The APU is retuned to whatever it reports
//!    ([`AudioPlayer::samples_rate`](crate::sound::AudioPlayer::samples_rate)
//!    feeds `blip.set_rates`), which is what keeps pitch correct — a mismatch
//!    between the reported rate and the real one is the *only* thing that makes
//!    a tone sharp or flat.

use crate::sound::sample_queue::SampleQueue;
use crate::sound::web_audio::{Scheduler, SchedulerStats, WebAudioPlayer};
use crate::sound::{host_status, AudioPlayer};
use std::sync::{Arc, Mutex};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{AudioContext, AudioContextState, GainNode};

/// Master attenuation applied past the end of emulation.
///
/// The same rule as the terminal frontend's volume control
/// (`sound::output_volume`): **playback volume is never NR50**. NR50 is
/// emulated behaviour the accuracy suites measure and games read back, so the
/// gain node sits *after* the APU has handed its samples over and cannot be
/// seen from inside the machine. 0.5 matches the desktop default (full scale
/// was the complaint there too).
const MASTER_GAIN: f32 = 0.5;

/// A live browser audio path: the context, the shared queue the APU fills, and
/// the scheduler that decides what to hand the context and when.
pub struct WebAudio {
    ctx: AudioContext,
    gain: GainNode,
    queue: Arc<Mutex<SampleQueue>>,
    sched: Scheduler,
    rate: u32,
}

impl WebAudio {
    /// Open a context and hand the machine its player.
    ///
    /// Returns `Err` with the browser's own reason, which the caller should
    /// report rather than swallow — see [`host_status`] for why "silent" and
    /// "broken" must never look the same.
    pub fn start(gb: &mut crate::gameboy::Gameboy) -> Result<WebAudio, JsValue> {
        let ctx = AudioContext::new()?;
        // The device's rate, not a guess. `sampleRate` is an f32 in the IDL and
        // always a whole number of hertz in practice.
        let rate = ctx.sample_rate().round().max(1.0) as u32;

        let gain = ctx.create_gain()?;
        gain.gain().set_value(MASTER_GAIN);
        gain.connect_with_audio_node(&ctx.destination())?;

        let (player, queue) = WebAudioPlayer::new(rate);
        let channels = 2u16;
        gb.enable_audio(Box::new(player) as Box<dyn AudioPlayer>);
        host_status::set_live(rate, channels);

        Ok(WebAudio {
            ctx,
            gain,
            queue,
            sched: Scheduler::new(rate),
            rate,
        })
    }

    /// `AudioContext.currentTime` — the clock the frontend paces emulation on.
    ///
    /// Deliberately the *same* clock the audio is scheduled against: pacing
    /// the producer on `performance.now()` (or on `requestAnimationFrame`'s
    /// timestamp) while the consumer runs on the audio clock leaves a residual
    /// drift between the two, and the queue then sheds a whole cushion's worth
    /// of samples every time it accumulates — an audible skip on a timer.
    /// One clock, no drift.
    pub fn now(&self) -> f64 {
        self.ctx.current_time()
    }

    /// Hand the context everything that is due. Call once per animation frame,
    /// after stepping the emulator.
    pub fn pump(&mut self) {
        let now = self.now();
        while let Some(block) = self.sched.next_block(now, &self.queue) {
            if let Err(e) = self.schedule(&block) {
                host_status::note_stream_error(describe(&e));
                return;
            }
        }
    }

    fn schedule(&self, block: &crate::sound::web_audio::Block) -> Result<(), JsValue> {
        let len = block.left.len().min(block.right.len()) as u32;
        let buffer = self.ctx.create_buffer(2, len, self.rate as f32)?;
        buffer.copy_to_channel(&block.left, 0)?;
        buffer.copy_to_channel(&block.right, 1)?;

        let source = self.ctx.create_buffer_source()?;
        source.set_buffer(Some(&buffer));
        source.connect_with_audio_node(&self.gain)?;
        // The node is dropped on this side the moment we return. That is
        // correct and is not a leak: a started `AudioBufferSourceNode` is kept
        // alive by the graph until it has finished playing and then collected
        // (the spec's "playing reference"), so nothing here has to hold or
        // free it.
        source.start_with_when(block.when)?;
        Ok(())
    }

    /// Re-`resume()` a suspended context. Cheap, idempotent, and safe to call
    /// from any input handler — see rule 1 in the module docs.
    pub fn nudge(&self) {
        if self.ctx.state() == AudioContextState::Suspended {
            // The promise is deliberately dropped: `resume()` either takes
            // effect or the context was not allowed to start yet, and the next
            // key press will try again.
            let _ = self.ctx.resume();
        }
    }

    /// True once the context is actually running — i.e. the browser let it
    /// start. False means silence with a *reason*, not a broken emulator.
    pub fn running(&self) -> bool {
        self.ctx.state() == AudioContextState::Running
    }

    /// Sample rate the context (and therefore the APU) is running at.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    /// What the scheduler has had to do. `underruns` is the number that says
    /// whether it crackled.
    pub fn stats(&self) -> SchedulerStats {
        self.sched.stats()
    }

    /// Seconds of audio scheduled and not yet heard — the latency a listener
    /// feels.
    pub fn lead(&self) -> f64 {
        self.sched.lead(self.now())
    }

    /// Samples waiting in the hand-off queue, and the drops the latency bound
    /// has had to make. A rising `dropped` means the producer is outrunning
    /// the audio clock.
    pub fn queue_depth_and_drops(&self) -> (usize, u64) {
        match self.queue.lock() {
            Ok(q) => (q.len(), q.stats.dropped),
            Err(_) => (0, 0),
        }
    }
}

/// A `JsValue` error as something printable. `JsValue` has no `Display`, and a
/// swallowed audio error is exactly the diagnostic hole `host_status` exists
/// to close.
fn describe(e: &JsValue) -> String {
    e.as_string()
        .or_else(|| {
            e.dyn_ref::<js_sys::Error>()
                .map(|err| String::from(err.message()))
        })
        .unwrap_or_else(|| format!("{e:?}"))
}
