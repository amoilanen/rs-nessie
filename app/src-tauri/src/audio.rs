//! [`cpal`]-backed implementation of [`nessie_runtime::AudioSink`].
//!
//! This module owns the desktop host's audio output path. The emulation
//! thread pushes mono `f32` samples at the APU's configured sample rate
//! (44.1 kHz by default — see [`APU_SAMPLE_RATE`]); the OS audio device's
//! preferred rate may differ (commonly 48 kHz). [`CpalAudio`] handles the
//! mismatch with a simple stateful linear resampler and a lock-free SPSC
//! ring buffer between the emulation thread and the cpal audio callback
//! (spec §5.3, §6.1).
//!
//! ## Design
//!
//! - The ring buffer ([`crossbeam_queue::ArrayQueue`]) holds device-rate
//!   samples. The emulation thread pushes via [`AudioSink::push_samples`];
//!   the cpal callback pops. Both ends are wait-free.
//! - **Drop-oldest on overrun.** When the ring is full, the push side
//!   discards the oldest sample to make room for the newest one. This is
//!   preferable to dropping the newest (the loop hears a stale "stutter"
//!   instead of a click) and never blocks the emulation thread (NFR-1).
//! - **Silence on underrun.** If the cpal callback finds the ring empty,
//!   it outputs zeros rather than stalling the OS audio thread.
//! - Volume and mute are atomics applied in the cpal callback so they take
//!   effect within one audio buffer (~10 ms) without locking.
//!
//! ## Audio thread
//!
//! `cpal::Stream` is `!Send` on macOS (CoreAudio constraint), but `AudioSink`
//! must be `Send + Sync`. We side-step the conflict by spawning a dedicated
//! "audio control" thread that builds, plays, and ultimately drops the
//! `Stream`. The host-facing [`CpalAudio`] only holds the lock-free shared
//! state (ring + atomics) and a shutdown flag.
//!
//! ## Resampling quality (FR-18)
//!
//! The linear resampler is a deliberate simplification documented in
//! `./docs/design-decisions.md`. It introduces a small amount of high-
//! frequency aliasing when downsampling from a higher device rate; a
//! follow-up step can swap in a polyphase / windowed-sinc resampler without
//! changing the public surface of this module.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_queue::ArrayQueue;
use parking_lot::Mutex;

use nessie_runtime::AudioSink;

use crate::error::{AppError, AppResult};

/// Sample rate (Hz) at which `nessie-core`'s APU produces audio by default.
/// Mirrors `nessie_core::apu::DEFAULT_SAMPLE_RATE`; duplicated here so the
/// host does not pull `nessie-core` directly into modules that only need
/// the audio glue.
pub const APU_SAMPLE_RATE: u32 = 44_100;

/// Ring buffer capacity in samples (at device rate).
///
/// Sized for roughly 250 ms of latency at 48 kHz — generous enough to
/// absorb scheduler hiccups on commodity hardware (NFR-1) but small enough
/// that audio stays in sync with video.
pub const RING_CAPACITY: usize = 12_000;

/// Lock-free state shared between the emulation thread (producer) and the
/// cpal audio callback (consumer).
///
/// Kept in a separate struct so the cpal callback can hold an `Arc` to it
/// while the rest of [`CpalAudio`] (resampler, join handle) stays
/// exclusively on the host side.
struct SharedAudioState {
    /// SPSC ring of resampled `f32` samples ready for the cpal callback.
    ring: ArrayQueue<f32>,
    /// Output volume in `0.0..=1.0`, stored as `f32::to_bits()`.
    volume_bits: AtomicU32,
    /// `true` when output is muted (callback writes zeros regardless of
    /// what the ring contains).
    muted: AtomicBool,
}

/// State scoped to the producer (emulation) side. The mutex is uncontested
/// in practice (only the emu thread calls [`AudioSink::push_samples`]); it
/// exists so [`CpalAudio`] can implement `Sync` cleanly.
struct PushState {
    resampler: LinearResampler,
}

/// Drives a [`cpal::Stream`] from the emulation thread without blocking it.
///
/// Construct one with [`CpalAudio::new`]. Push samples via the
/// [`AudioSink`] impl. Use [`CpalAudio::set_volume`] / [`CpalAudio::set_muted`]
/// from any thread for live volume control.
pub struct CpalAudio {
    inner: Arc<SharedAudioState>,
    push: Mutex<PushState>,
    source_sample_rate: u32,
    device_sample_rate: u32,
    /// Set to `true` on drop; the audio thread polls this and exits, which
    /// drops the `cpal::Stream` and closes the OS audio device.
    shutdown: Arc<AtomicBool>,
    audio_thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for CpalAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CpalAudio")
            .field("source_sample_rate", &self.source_sample_rate)
            .field("device_sample_rate", &self.device_sample_rate)
            .field("ring_len", &self.inner.ring.len())
            .field("ring_capacity", &self.inner.ring.capacity())
            .field(
                "volume",
                &f32::from_bits(self.inner.volume_bits.load(Ordering::Relaxed)),
            )
            .field("muted", &self.inner.muted.load(Ordering::Relaxed))
            .finish()
    }
}

impl CpalAudio {
    /// Open the default OS output device and start an audio stream.
    ///
    /// `source_sample_rate` is the rate at which the APU produces samples
    /// (typically [`APU_SAMPLE_RATE`]). The device's preferred sample rate
    /// is reported back through [`AudioSink::sample_rate`].
    ///
    /// On failure (no audio device, no compatible format, OS refused stream
    /// creation) returns [`AppError::Io`] with a descriptive message — the
    /// host should fall back to muted operation rather than panicking.
    pub fn new(source_sample_rate: u32) -> AppResult<Self> {
        assert!(source_sample_rate > 0, "source_sample_rate must be > 0");
        let inner = Arc::new(SharedAudioState {
            ring: ArrayQueue::new(RING_CAPACITY),
            volume_bits: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
        });
        let shutdown = Arc::new(AtomicBool::new(false));

        let (init_tx, init_rx) = mpsc::channel::<AppResult<u32>>();
        let inner_clone = Arc::clone(&inner);
        let shutdown_clone = Arc::clone(&shutdown);

        let audio_thread = thread::Builder::new()
            .name("nessie-audio".into())
            .spawn(move || {
                audio_thread_main(inner_clone, shutdown_clone, init_tx);
            })
            .map_err(|e| AppError::Io(format!("failed to spawn audio thread: {e}")))?;

        // Block briefly for the audio thread to either report success or fail
        // with a descriptive error. The audio thread always sends exactly one
        // message before transitioning to its park loop.
        let device_sample_rate = match init_rx.recv() {
            Ok(result) => result?,
            Err(_) => {
                // Audio thread panicked before sending. Best effort to join
                // and surface a recognizable error.
                shutdown.store(true, Ordering::Release);
                let _ = audio_thread.join();
                return Err(AppError::Io(
                    "audio thread exited before reporting device sample rate".into(),
                ));
            }
        };

        Ok(Self {
            inner,
            push: Mutex::new(PushState {
                resampler: LinearResampler::new(source_sample_rate, device_sample_rate),
            }),
            source_sample_rate,
            device_sample_rate,
            shutdown,
            audio_thread: Some(audio_thread),
        })
    }

    /// Update the output volume.
    ///
    /// Out-of-range values (including NaN) are clamped to `0.0..=1.0` rather
    /// than rejected — `Settings::set_volume` is the validation gate; this
    /// is the runtime applicator and must never fail.
    pub fn set_volume(&self, volume: f32) {
        let clamped = if volume.is_nan() {
            0.0
        } else {
            volume.clamp(0.0, 1.0)
        };
        self.inner
            .volume_bits
            .store(clamped.to_bits(), Ordering::Release);
    }

    /// Mute or un-mute the output. Effective within one audio buffer
    /// (~10 ms on typical OSes).
    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.store(muted, Ordering::Release);
    }

    /// Source sample rate this sink was configured with.
    #[must_use]
    pub fn source_sample_rate(&self) -> u32 {
        self.source_sample_rate
    }

    /// Device sample rate as reported by cpal at open time.
    #[must_use]
    pub fn device_sample_rate(&self) -> u32 {
        self.device_sample_rate
    }
}

impl AudioSink for CpalAudio {
    fn push_samples(&self, samples: &[f32]) {
        let mut push = self.push.lock();
        push_resampled_into_ring(&mut push.resampler, &self.inner.ring, samples);
    }

    fn sample_rate(&self) -> u32 {
        self.device_sample_rate
    }
}

impl Drop for CpalAudio {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(handle) = self.audio_thread.take() {
            // Wake the audio thread immediately so we do not pay the park
            // timeout when shutting down a session.
            handle.thread().unpark();
            if let Err(err) = handle.join() {
                log::error!("nessie-audio thread panicked on shutdown: {err:?}");
            }
        }
    }
}

/// Push `samples` through `resampler` into `ring`, dropping the oldest queued
/// sample whenever the ring is full.
///
/// Public to the crate so [`crate::session`] and the unit tests can exercise
/// the producer side without standing up a real cpal stream.
pub(crate) fn push_resampled_into_ring(
    resampler: &mut LinearResampler,
    ring: &ArrayQueue<f32>,
    samples: &[f32],
) {
    resampler.process(samples, |out| {
        push_drop_oldest(ring, out);
    });
}

/// Push a single sample, evicting the oldest queued sample on overrun.
///
/// Crate-public so the unit tests can verify the eviction policy without
/// relying on cpal.
pub(crate) fn push_drop_oldest(ring: &ArrayQueue<f32>, sample: f32) {
    if ring.push(sample).is_err() {
        let _ = ring.pop();
        let _ = ring.push(sample);
    }
}

/// Body of the dedicated audio thread.
///
/// Builds the cpal output stream, plays it, then parks waiting for shutdown.
/// All cpal interaction stays on this thread because [`cpal::Stream`] is
/// `!Send` on macOS.
fn audio_thread_main(
    inner: Arc<SharedAudioState>,
    shutdown: Arc<AtomicBool>,
    init_tx: mpsc::Sender<AppResult<u32>>,
) {
    let stream_result = build_output_stream(&inner);
    match stream_result {
        Ok((stream, device_rate)) => {
            // Report success before transitioning to play / park. If the
            // host has already dropped the receiver (e.g. it crashed during
            // `CpalAudio::new`) we still try to play so the user hears
            // something; we'll exit on the next shutdown poll.
            let _ = init_tx.send(Ok(device_rate));
            if let Err(err) = stream.play() {
                log::error!("failed to start cpal output stream: {err}");
                return;
            }
            while !shutdown.load(Ordering::Acquire) {
                // Polling at 250 ms is good enough: the cpal callback runs
                // on its own OS-managed thread, and a session shutdown
                // unparks us explicitly anyway.
                thread::park_timeout(Duration::from_millis(250));
            }
            // Drop the stream explicitly so the OS audio device is released
            // before the thread exits. This is also what happens implicitly
            // when the function returns, but spelling it out makes the
            // teardown order obvious.
            drop(stream);
        }
        Err(err) => {
            let _ = init_tx.send(Err(err));
        }
    }
}

/// Open the default output device and build an f32 output stream sourced
/// from `inner.ring`. Returns the configured device sample rate alongside
/// the stream.
fn build_output_stream(inner: &Arc<SharedAudioState>) -> AppResult<(cpal::Stream, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| AppError::Io("no default audio output device available".into()))?;

    let (config, _format) = pick_output_config(&device)?;
    let device_rate = config.sample_rate.0;
    let channels = config.channels as usize;
    if channels == 0 {
        return Err(AppError::Io("audio device reports zero channels".into()));
    }

    let cb_state = Arc::clone(inner);
    let err_fn = |err| {
        log::error!("cpal stream error: {err}");
    };

    let stream = device
        .build_output_stream::<f32, _, _>(
            &config,
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let muted = cb_state.muted.load(Ordering::Acquire);
                let vol = f32::from_bits(cb_state.volume_bits.load(Ordering::Acquire));
                // Walk frame-by-frame so we duplicate the mono APU sample
                // into every channel the device wants (stereo, surround,
                // …). Drains the ring whether or not we're muted so a long
                // mute period does not desynchronize playback when the
                // user un-mutes.
                for frame in data.chunks_mut(channels) {
                    let raw = cb_state.ring.pop().unwrap_or(0.0);
                    let out = if muted { 0.0 } else { raw * vol };
                    for sample in frame.iter_mut() {
                        *sample = out;
                    }
                }
            },
            err_fn,
            None,
        )
        .map_err(|e| AppError::Io(format!("failed to build cpal output stream: {e}")))?;

    Ok((stream, device_rate))
}

/// Pick a stream config: prefer the device default if it is f32 already,
/// otherwise scan supported configs for the first f32 option.
fn pick_output_config(device: &cpal::Device) -> AppResult<(StreamConfig, SampleFormat)> {
    let default = device
        .default_output_config()
        .map_err(|e| AppError::Io(format!("failed to query default output config: {e}")))?;
    if default.sample_format() == SampleFormat::F32 {
        let format = default.sample_format();
        return Ok((default.config(), format));
    }
    let mut supported = device
        .supported_output_configs()
        .map_err(|e| AppError::Io(format!("failed to query supported configs: {e}")))?
        .filter(|c| c.sample_format() == SampleFormat::F32);
    let candidate = supported
        .next()
        .ok_or_else(|| AppError::Io("no f32 output config available on default device".into()))?;
    // Prefer 44.1 kHz when reachable so the resampler is a passthrough on
    // the common APU rate. Fall back to 48 kHz, then to the range minimum.
    let min = candidate.min_sample_rate();
    let max = candidate.max_sample_rate();
    let pick = [cpal::SampleRate(44_100), cpal::SampleRate(48_000)]
        .into_iter()
        .find(|r| *r >= min && *r <= max)
        .unwrap_or(min);
    let config = candidate.with_sample_rate(pick);
    let format = config.sample_format();
    Ok((config.config(), format))
}

/// Streaming linear interpolation resampler.
///
/// Maintains the last seen input sample so interpolation works correctly
/// across calls to [`process`](LinearResampler::process). Uses a `u32`
/// accumulator in source-rate units to stay deterministic and avoid any
/// floating-point error drift over long sessions.
///
/// Cost is one multiply-add per emitted sample.
pub(crate) struct LinearResampler {
    src_rate: u32,
    dst_rate: u32,
    /// Phase accumulator in source-rate units. Invariant: `acc < src_rate`.
    acc: u32,
    /// Last input sample we saw, used as the left endpoint of interpolation
    /// for the first output that falls between this and the next input.
    prev: f32,
    /// `false` until the first input sample is fed; output samples generated
    /// before the first input are silence (used when device rate > 0 and we
    /// fall behind the audio thread on startup).
    has_prev: bool,
}

impl LinearResampler {
    pub(crate) fn new(src_rate: u32, dst_rate: u32) -> Self {
        assert!(src_rate > 0 && dst_rate > 0, "resampler rates must be > 0");
        Self {
            src_rate,
            dst_rate,
            acc: 0,
            prev: 0.0,
            has_prev: false,
        }
    }

    /// Feed `input` and call `emit` once per produced output sample.
    ///
    /// Invariant maintained across calls: `self.acc < self.src_rate` on
    /// entry. We accumulate `dst_rate` per input sample (positional units
    /// of `src_rate`), then emit one output for each boundary the phase
    /// crosses inside the current input frame, interpolating at the
    /// fractional position between `prev` and `cur` where the boundary
    /// falls. Doing all arithmetic in terms of boundary positions (rather
    /// than "subtract one src_rate at a time") avoids unsigned underflow
    /// when `dst_rate >= 2 * src_rate` would force the while loop to run
    /// twice.
    pub(crate) fn process<F: FnMut(f32)>(&mut self, input: &[f32], mut emit: F) {
        for &cur in input {
            if !self.has_prev {
                self.prev = cur;
                self.has_prev = true;
            }
            let acc_old = self.acc;
            // `acc_old < src_rate` and we add `dst_rate`. For typical NES
            // → device rates (44.1 kHz → 48 kHz, 96 kHz, …) this stays
            // well below `u32::MAX`.
            let acc_new = acc_old.saturating_add(self.dst_rate);
            // Number of src_rate boundaries crossed by going from
            // `acc_old` (< src_rate) to `acc_new` is just
            // `floor(acc_new / src_rate)`.
            let crossings = acc_new / self.src_rate;
            for n in 1..=crossings {
                let boundary = n.saturating_mul(self.src_rate);
                // `boundary >= src_rate > acc_old`, so the subtraction is
                // always positive.
                let t = (boundary - acc_old) as f32 / self.dst_rate as f32;
                let out = self.prev * (1.0 - t) + cur * t;
                emit(out);
            }
            self.acc = acc_new % self.src_rate;
            self.prev = cur;
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn push_drop_oldest_evicts_the_oldest_sample_when_full() {
        // Build a small ring so we can exhaustively check the eviction
        // policy described in the module docs.
        let ring: ArrayQueue<f32> = ArrayQueue::new(4);
        for s in [1.0, 2.0, 3.0, 4.0] {
            push_drop_oldest(&ring, s);
        }
        assert_eq!(ring.len(), 4);
        // Two more samples force eviction of the oldest two.
        push_drop_oldest(&ring, 5.0);
        push_drop_oldest(&ring, 6.0);
        assert_eq!(ring.len(), 4);
        // Drain in FIFO order: the survivors must be the four newest.
        let mut drained = Vec::with_capacity(4);
        while let Some(s) = ring.pop() {
            drained.push(s);
        }
        assert_eq!(drained, vec![3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn push_resampled_into_ring_drops_oldest_on_overflow() {
        // End-to-end check on the producer pipeline: send more samples than
        // the ring's capacity through a passthrough resampler (src==dst) and
        // confirm the ring ends up with the newest samples, not the oldest.
        let ring: ArrayQueue<f32> = ArrayQueue::new(3);
        let mut resampler = LinearResampler::new(44_100, 44_100);
        let input: Vec<f32> = (1..=10).map(|i| i as f32).collect();
        push_resampled_into_ring(&mut resampler, &ring, &input);
        assert_eq!(ring.len(), 3);
        let mut drained = Vec::with_capacity(3);
        while let Some(s) = ring.pop() {
            drained.push(s);
        }
        // Last three pushed samples win; the first seven were evicted.
        assert_eq!(drained, vec![8.0, 9.0, 10.0]);
    }

    #[test]
    fn linear_resampler_passthrough_when_rates_match() {
        let mut r = LinearResampler::new(44_100, 44_100);
        let mut out = Vec::new();
        r.process(&[0.1, 0.2, 0.3, 0.4], |s| out.push(s));
        assert_eq!(out.len(), 4);
        for (got, want) in out.iter().zip([0.1f32, 0.2, 0.3, 0.4].iter()) {
            assert!((got - want).abs() < 1e-6, "got {got}, want {want}");
        }
    }

    #[test]
    fn linear_resampler_upsamples_to_higher_rate() {
        // 44.1 kHz → 48 kHz: 48000/44100 ≈ 1.088 output samples per input.
        // Over 1000 input samples we expect ~1088 outputs.
        let mut r = LinearResampler::new(44_100, 48_000);
        let mut count = 0usize;
        r.process(&vec![0.0_f32; 1_000], |_| count += 1);
        assert!(
            (1085..=1090).contains(&count),
            "expected ~1088 outputs, got {count}"
        );
    }

    #[test]
    fn linear_resampler_downsamples_to_lower_rate() {
        // 48 kHz → 44.1 kHz: roughly 0.919 outputs per input.
        let mut r = LinearResampler::new(48_000, 44_100);
        let mut count = 0usize;
        r.process(&vec![0.0_f32; 1_000], |_| count += 1);
        assert!(
            (916..=920).contains(&count),
            "expected ~918 outputs, got {count}"
        );
    }

    #[test]
    fn linear_resampler_state_is_preserved_across_calls() {
        // Output count over two halves of an input buffer must equal the
        // single-shot count over the whole buffer (state continuity).
        let mut r1 = LinearResampler::new(44_100, 48_000);
        let mut total1 = 0usize;
        let buf: Vec<f32> = (0..1_000).map(|i| (i as f32) * 0.001).collect();
        r1.process(&buf, |_| total1 += 1);

        let mut r2 = LinearResampler::new(44_100, 48_000);
        let mut total2 = 0usize;
        r2.process(&buf[..500], |_| total2 += 1);
        r2.process(&buf[500..], |_| total2 += 1);
        assert_eq!(total1, total2);
    }

    #[test]
    fn cpal_audio_new_returns_io_error_when_no_device_or_succeeds() {
        // Don't fail CI on headless runners (no audio device): just check we
        // never panic and we return either Ok or AppError::Io.
        match CpalAudio::new(APU_SAMPLE_RATE) {
            Ok(audio) => {
                // Smoke-check the surface area without actually playing
                // anything that would be audible.
                assert_eq!(audio.sample_rate(), audio.device_sample_rate());
                assert_eq!(audio.source_sample_rate(), APU_SAMPLE_RATE);
                audio.set_volume(0.5);
                audio.set_volume(f32::NAN); // must not panic
                audio.set_muted(true);
                audio.set_muted(false);
                // Pushing samples must not block even when the ring is well
                // beyond capacity (drop-oldest policy).
                let huge = vec![0.123_f32; RING_CAPACITY * 3];
                audio.push_samples(&huge);
                // Dropping releases the audio thread cleanly.
                drop(audio);
            }
            Err(AppError::Io(_)) => {
                // Expected on headless CI; the module still loaded.
            }
            Err(other) => panic!("unexpected error type: {other:?}"),
        }
    }
}
