//! [`Session`] — the per-game emulation thread owner.
//!
//! Spec §6.1 calls for a dedicated OS thread that owns the [`Nes`], reads
//! input from atomics, steps one NTSC frame, pushes the rendered framebuffer
//! through a [`FrameSink`], drains audio samples into an [`AudioSink`], and
//! sleeps until the next frame deadline as scheduled by [`FramePacer`].
//!
//! `Session` is the host's handle to that thread. It exposes:
//!
//! - `start` / `start_with` — spawn the thread.
//! - `set_button` — write a button state through an `AtomicU8` per player
//!   (one bit per [`Button`] in the canonical NES order).
//! - `stop` — request shutdown and join the thread. Idempotent.
//!
//! Inputs are written by the host from any thread and read by the emulation
//! thread once per frame *before* `step_frame()`, matching real-hardware
//! polling cadence.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use nessie_core::{Button, Nes, Player};
use parking_lot::Mutex;

use crate::clock::{Clock, SystemClock};
use crate::pacer::FramePacer;
use crate::sink::{AudioSink, FrameSink, FRAME_BYTES};

/// Shared slot through which the emulation thread surfaces its final
/// battery-backed PRG-RAM snapshot before exiting. The host fills this with
/// a fresh `Arc<Mutex<None>>`, hands it to [`SessionOptions::battery_out`],
/// and after [`Session::stop`] joins reads back the latest snapshot (or
/// `None` for non-battery cartridges).
///
/// This is intentionally a coarse-grained channel — battery saves are
/// written at most a couple of times per session (on stop / shutdown), so an
/// `Arc<Mutex<…>>` is cheaper than wiring up a real channel.
pub type BatteryOut = Arc<Mutex<Option<Vec<u8>>>>;

/// Knobs callers can tweak when starting a session.
///
/// Defaults are tuned for the desktop shell (real-time pacing, no frame
/// cap); integration tests typically disable pacing and set
/// `stop_after_frames` so the thread terminates deterministically.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// When `true` (the default), the emulation thread sleeps to keep ≈60 Hz
    /// real-time pacing. When `false`, the thread runs `Nes::step_frame` as
    /// fast as it can — used by tests and headless benchmarks.
    pub paced: bool,

    /// If `Some(n)`, the emulation thread exits cleanly after submitting
    /// `n` frames. Test-only knob; the desktop shell leaves this `None`.
    pub stop_after_frames: Option<u64>,

    /// If the pacer detects we are this many frames behind real time it
    /// snaps forward instead of trying to render every dropped frame.
    pub catch_up_threshold: u64,

    /// Upper bound on a single catch-up step. Defends against laptops that
    /// resume after a long suspend with a multi-hour clock jump.
    pub catch_up_max_skip: u64,

    /// Optional sink for the cartridge's battery-backed PRG-RAM. When `Some`,
    /// the emulation thread writes [`Nes::battery_snapshot`] into the slot
    /// just before exiting (and on cartridges without battery, leaves the
    /// slot at `None`). Hosts use this to persist `.srm` files after a
    /// session ends.
    pub battery_out: Option<BatteryOut>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            paced: true,
            stop_after_frames: None,
            // Allow a generous 4-frame buffer before we declare a stall.
            catch_up_threshold: 4,
            // 60 frames ≈ 1 s of stall is the most we'll skip in one go.
            catch_up_max_skip: 60,
            battery_out: None,
        }
    }
}

/// Handle to a running emulation thread.
pub struct Session {
    shutdown: Arc<AtomicBool>,
    inputs: [Arc<AtomicU8>; 2],
    join: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("running", &self.join.is_some())
            .field("shutdown_requested", &self.shutdown.load(Ordering::Relaxed))
            .finish()
    }
}

impl Session {
    /// Spawn the emulation thread with default options ([`SessionOptions`])
    /// and a [`SystemClock`].
    pub fn start<A, F>(nes: Nes, audio: Arc<A>, frame: Arc<F>) -> Self
    where
        A: AudioSink + 'static,
        F: FrameSink + 'static,
    {
        Self::start_with(nes, audio, frame, SessionOptions::default(), SystemClock)
    }

    /// Spawn the emulation thread with custom options and a custom clock.
    pub fn start_with<A, F, C>(
        nes: Nes,
        audio: Arc<A>,
        frame: Arc<F>,
        options: SessionOptions,
        clock: C,
    ) -> Self
    where
        A: AudioSink + 'static,
        F: FrameSink + 'static,
        C: Clock + 'static,
    {
        let shutdown = Arc::new(AtomicBool::new(false));
        let inputs: [Arc<AtomicU8>; 2] = [Arc::new(AtomicU8::new(0)), Arc::new(AtomicU8::new(0))];

        let join = {
            let shutdown = Arc::clone(&shutdown);
            let inputs = [Arc::clone(&inputs[0]), Arc::clone(&inputs[1])];
            // `Builder::spawn` only fails when the OS refuses to create a
            // thread (typically: process out of file descriptors / TIDs). At
            // that point the host has no realistic recovery path, so we
            // surface the failure as a panic with a descriptive message.
            #[allow(clippy::expect_used)]
            thread::Builder::new()
                .name("nessie-emu".into())
                .spawn(move || {
                    run_emulation(nes, audio, frame, options, clock, shutdown, inputs);
                })
                .expect("failed to spawn nessie-emu thread")
        };

        Self {
            shutdown,
            inputs,
            join: Some(join),
        }
    }

    /// Forward a key event to the emulation thread. The new bitmap is read
    /// before each `step_frame()` so the change is visible within ≤16.7 ms.
    pub fn set_button(&self, player: Player, button: Button, pressed: bool) {
        let mask = 1u8 << button.bit();
        let atomic = &self.inputs[player.index()];
        if pressed {
            atomic.fetch_or(mask, Ordering::Release);
        } else {
            atomic.fetch_and(!mask, Ordering::Release);
        }
    }

    /// Read back the live bitmap for a player. Mostly useful in tests.
    #[must_use]
    pub fn button_state(&self, player: Player) -> u8 {
        self.inputs[player.index()].load(Ordering::Acquire)
    }

    /// Signal the emulation thread to exit and block until it joins.
    /// Idempotent — calling `stop` twice is fine and the second call is a
    /// no-op.
    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.join.take() {
            // Use `join` so any panics inside the thread are propagated. The
            // host should never see this in normal operation; if it does,
            // the panic message in the log is the diagnostic we want.
            if let Err(err) = handle.join() {
                log::error!("nessie-emu thread panicked: {err:?}");
            }
        }
    }

    /// True iff the emulation thread is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.join.as_ref().is_some_and(|h| !h.is_finished())
    }

    /// Wait for the thread to exit on its own (e.g. due to
    /// `stop_after_frames`) and join it. Returns an error string if the
    /// thread panicked.
    pub fn wait(&mut self) -> Result<(), String> {
        if let Some(handle) = self.join.take() {
            handle
                .join()
                .map_err(|e| format!("nessie-emu panicked: {e:?}"))?;
        }
        Ok(())
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // Best-effort cleanup so `Session` is RAII-safe.
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.join.take() {
            let _ = handle.join();
        }
    }
}

/// Body of the emulation thread.
fn run_emulation<A, F, C>(
    mut nes: Nes,
    audio: Arc<A>,
    frame: Arc<F>,
    options: SessionOptions,
    clock: C,
    shutdown: Arc<AtomicBool>,
    inputs: [Arc<AtomicU8>; 2],
) where
    A: AudioSink,
    F: FrameSink,
    C: Clock,
{
    // Hold on to the optional battery sink for the duration of the loop;
    // updated once after the loop exits so hosts can persist a `.srm` file.
    let battery_out = options.battery_out.clone();
    let sample_rate = audio.sample_rate();
    let mut pacer = FramePacer::new(clock, sample_rate);
    let mut audio_buf: Vec<f32> = Vec::with_capacity(sample_rate as usize / 30);
    // Per-player last-seen bitmaps so we only push *transitions* into the
    // `Nes` (cheaper than re-issuing all 8 buttons every frame).
    let mut last_inputs = [0u8; 2];
    let mut submitted: u64 = 0;

    while !shutdown.load(Ordering::Acquire) {
        if let Some(limit) = options.stop_after_frames {
            if submitted >= limit {
                break;
            }
        }

        // 1. Drain pending button changes into the Nes.
        apply_inputs(&mut nes, &inputs, &mut last_inputs);

        // 2. Step one NTSC frame.
        nes.step_frame();

        // 3. Push audio.
        audio_buf.clear();
        nes.drain_audio(&mut audio_buf);
        if !audio_buf.is_empty() {
            audio.push_samples(&audio_buf);
        }

        // 4. Submit framebuffer. `framebuffer()` returns a reference into the
        //    PPU; sinks that need to keep it must copy.
        let fb: &[u8; FRAME_BYTES] = nes.framebuffer();
        frame.submit(fb, submitted);
        submitted = submitted.saturating_add(1);

        // 5. Pace.
        pacer.advance();
        if options.paced {
            // First handle gross stalls (e.g. laptop sleep) so we do not
            // emit a flood of catch-up frames.
            pacer.catch_up(options.catch_up_threshold, options.catch_up_max_skip);
            let wait = pacer.wait_for_next_frame();
            if wait > Duration::ZERO {
                // `park_timeout` would be lighter but `Session::stop` does not
                // currently un-park; sleep is fine for ≤16 ms granularity.
                thread::sleep(wait);
            }
        }
    }

    // Final battery snapshot, if a host requested one. We do this *after* the
    // loop so the snapshot reflects every write the cartridge issued during
    // the run (including the last frame). `None` for non-battery carts.
    if let Some(out) = battery_out {
        *out.lock() = nes.battery_snapshot();
    }
}

/// Diff the incoming bitmaps against the last-seen ones and issue
/// `set_button` calls only for changed bits. Keeps the controller's shift
/// register from being thrashed every frame.
fn apply_inputs(nes: &mut Nes, inputs: &[Arc<AtomicU8>; 2], last: &mut [u8; 2]) {
    for (idx, atomic) in inputs.iter().enumerate() {
        let now = atomic.load(Ordering::Acquire);
        let prev = last[idx];
        if now == prev {
            continue;
        }
        let changed = now ^ prev;
        let player = if idx == 0 { Player::One } else { Player::Two };
        for bit in 0..8u8 {
            if changed & (1 << bit) == 0 {
                continue;
            }
            let button = button_from_bit(bit);
            let pressed = now & (1 << bit) != 0;
            nes.set_button(player, button, pressed);
        }
        last[idx] = now;
    }
}

#[inline]
fn button_from_bit(bit: u8) -> Button {
    match bit {
        0 => Button::A,
        1 => Button::B,
        2 => Button::Select,
        3 => Button::Start,
        4 => Button::Up,
        5 => Button::Down,
        6 => Button::Left,
        _ => Button::Right,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    use super::*;
    use nessie_core::Nes;

    /// Minimal NROM ROM whose reset vector points at an infinite loop at
    /// `$C000`. Mirrors the helper in `nessie-core::nes::tests` but copied
    /// here so the runtime crate has no cross-crate `#[cfg(test)]` exposure.
    fn idle_rom() -> Vec<u8> {
        let mut rom = Vec::with_capacity(16 + 16 * 1024 + 8 * 1024);
        rom.extend_from_slice(b"NES\x1a");
        rom.push(1); // 16 KB PRG
        rom.push(1); // 8 KB CHR
        rom.push(0);
        rom.push(0);
        rom.extend_from_slice(&[0u8; 8]);
        rom.extend(std::iter::repeat(0u8).take(16 * 1024));
        rom[16 + 0x3FFC] = 0x00;
        rom[16 + 0x3FFD] = 0xC0;
        rom[16] = 0x4C;
        rom[17] = 0x00;
        rom[18] = 0xC0;
        rom.extend(std::iter::repeat(0u8).take(8 * 1024));
        rom
    }

    struct CountingAudioSink {
        samples: AtomicUsize,
        rate: u32,
    }

    impl AudioSink for CountingAudioSink {
        fn push_samples(&self, samples: &[f32]) {
            self.samples.fetch_add(samples.len(), Ordering::Relaxed);
        }
        fn sample_rate(&self) -> u32 {
            self.rate
        }
    }

    struct CountingFrameSink {
        frames: AtomicUsize,
        last_index: parking_lot::Mutex<Option<u64>>,
    }

    impl FrameSink for CountingFrameSink {
        fn submit(&self, _frame: &[u8; FRAME_BYTES], frame_index: u64) {
            self.frames.fetch_add(1, Ordering::Relaxed);
            *self.last_index.lock() = Some(frame_index);
        }
    }

    fn run_short_session(frames: u64) -> (Arc<CountingAudioSink>, Arc<CountingFrameSink>) {
        let nes = Nes::from_ines(&idle_rom()).unwrap();
        let audio = Arc::new(CountingAudioSink {
            samples: AtomicUsize::new(0),
            rate: 44_100,
        });
        let frame = Arc::new(CountingFrameSink {
            frames: AtomicUsize::new(0),
            last_index: parking_lot::Mutex::new(None),
        });
        let opts = SessionOptions {
            paced: false,
            stop_after_frames: Some(frames),
            ..Default::default()
        };
        let mut session = Session::start_with(
            nes,
            Arc::clone(&audio),
            Arc::clone(&frame),
            opts,
            SystemClock,
        );
        session.wait().expect("session thread should not panic");
        (audio, frame)
    }

    #[test]
    fn session_submits_exactly_the_requested_number_of_frames() {
        let (_audio, frame) = run_short_session(10);
        assert_eq!(frame.frames.load(Ordering::Relaxed), 10);
        assert_eq!(*frame.last_index.lock(), Some(9));
    }

    #[test]
    fn session_pushes_non_zero_audio_samples() {
        let (audio, _frame) = run_short_session(10);
        assert!(
            audio.samples.load(Ordering::Relaxed) > 0,
            "expected the APU to produce samples"
        );
    }

    #[test]
    fn set_button_writes_through_to_the_atomic_state() {
        let nes = Nes::from_ines(&idle_rom()).unwrap();
        let audio = Arc::new(CountingAudioSink {
            samples: AtomicUsize::new(0),
            rate: 44_100,
        });
        let frame = Arc::new(CountingFrameSink {
            frames: AtomicUsize::new(0),
            last_index: parking_lot::Mutex::new(None),
        });
        let opts = SessionOptions {
            paced: false,
            // Long enough to let the test poke buttons before shutdown.
            stop_after_frames: Some(2),
            ..Default::default()
        };
        let mut session = Session::start_with(nes, audio, frame, opts, SystemClock);
        session.set_button(Player::One, Button::A, true);
        session.set_button(Player::Two, Button::Start, true);
        assert_eq!(session.button_state(Player::One) & 0x01, 0x01);
        assert_eq!(session.button_state(Player::Two) & 0x08, 0x08);
        session.set_button(Player::One, Button::A, false);
        assert_eq!(session.button_state(Player::One) & 0x01, 0);
        session.stop();
        assert!(!session.is_running());
    }

    #[test]
    fn stop_is_idempotent() {
        let nes = Nes::from_ines(&idle_rom()).unwrap();
        let audio = Arc::new(CountingAudioSink {
            samples: AtomicUsize::new(0),
            rate: 44_100,
        });
        let frame = Arc::new(CountingFrameSink {
            frames: AtomicUsize::new(0),
            last_index: parking_lot::Mutex::new(None),
        });
        let mut session = Session::start_with(
            nes,
            audio,
            frame,
            SessionOptions {
                paced: false,
                stop_after_frames: Some(1),
                ..Default::default()
            },
            SystemClock,
        );
        session.stop();
        session.stop(); // must not panic / double-join.
    }
}
