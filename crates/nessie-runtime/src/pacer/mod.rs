//! Frame pacer: schedules the next NTSC frame's deadline.
//!
//! Spec §6.1 calls for "video follows audio" — the audio sink's reported
//! sample rate is the runtime's reference clock. We do **not** loop on the
//! audio device's playback position directly (that would couple this crate
//! to a specific audio backend); instead the pacer derives a per-frame
//! deadline from the sample-rate-implied frame period and a host-provided
//! [`Clock`].
//!
//! The pacer is intentionally tiny:
//!
//! 1. [`FramePacer::new`] records a `start` instant.
//! 2. [`FramePacer::wait_for_next_frame`] returns how long the caller should
//!    sleep before rendering the next frame.
//! 3. [`FramePacer::advance`] bumps the internal frame index after the caller
//!    has rendered a frame.
//! 4. [`FramePacer::catch_up`] skips the internal index forward if real time
//!    has run past the pacer's view of "now" by more than a configurable
//!    threshold — this prevents the emulation thread from over-running and
//!    spitting out a flood of frames after a stall (OS scheduling jitter,
//!    a debugger break, the laptop closing the lid, etc.).

use std::time::{Duration, Instant};

use crate::clock::Clock;

#[cfg(test)]
mod tests;

/// Canonical NTSC frame rate. The real NES runs at 60.0988 Hz; the runtime
/// uses that exact value so audio drift accumulates as slowly as possible.
pub const NTSC_FPS: f64 = 60.098_813_897_440_55;

/// Schedules the next frame deadline based on an audio sink's sample rate.
///
/// The pacer is generic over the [`Clock`] implementation so unit tests can
/// drive it from a fake clock without sleeping. The desktop host uses
/// [`SystemClock`](crate::SystemClock).
#[derive(Debug)]
pub struct FramePacer<C: Clock> {
    clock: C,
    sample_rate: u32,
    /// Frame period in floating-point seconds. We compute it once at
    /// construction so per-tick math is a single `mul` + `add`.
    frame_period_secs: f64,
    /// The "time zero" the pacer measures deadlines from. Set on construction
    /// and re-synced by [`FramePacer::catch_up`] when we fall behind.
    start: Instant,
    /// Index of the frame that has been rendered most recently. `0` means no
    /// frame has been rendered yet.
    frame_index: u64,
}

impl<C: Clock> FramePacer<C> {
    /// Build a pacer with the supplied clock and audio sample rate.
    ///
    /// The sample rate is informational: only the frame period (1 / FPS) is
    /// actually consulted when computing deadlines. The argument exists so
    /// callers wire the audio sink's `sample_rate()` straight through and
    /// the design can be evolved to use the audio sink's actual playback
    /// position as the clock later without changing the public API.
    ///
    /// # Panics
    ///
    /// Panics if `sample_rate == 0` — the runtime requires a real audio sink.
    #[must_use]
    pub fn new(clock: C, sample_rate: u32) -> Self {
        assert!(sample_rate > 0, "sample_rate must be positive");
        let start = clock.now();
        Self {
            clock,
            sample_rate,
            frame_period_secs: 1.0 / NTSC_FPS,
            start,
            frame_index: 0,
        }
    }

    /// Audio sample rate this pacer was built with.
    #[inline]
    #[must_use]
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Index of the most recently rendered frame (0 if none yet).
    #[inline]
    #[must_use]
    pub fn frame_index(&self) -> u64 {
        self.frame_index
    }

    /// The wall-clock deadline by which frame `index` should have completed.
    #[must_use]
    pub fn deadline_for(&self, index: u64) -> Instant {
        let secs = (index as f64) * self.frame_period_secs;
        self.start + Duration::from_secs_f64(secs)
    }

    /// How long the caller should sleep before rendering frame
    /// `frame_index + 1`. Returns [`Duration::ZERO`] if the deadline has
    /// already passed.
    #[must_use]
    pub fn wait_for_next_frame(&self) -> Duration {
        let deadline = self.deadline_for(self.frame_index + 1);
        let now = self.clock.now();
        if deadline > now {
            deadline - now
        } else {
            Duration::ZERO
        }
    }

    /// Mark the current frame as rendered and return the new frame index.
    #[inline]
    pub fn advance(&mut self) -> u64 {
        self.frame_index = self.frame_index.saturating_add(1);
        self.frame_index
    }

    /// If real time has run past the next-frame deadline by `threshold` or
    /// more frames, skip the pacer forward so the emulation thread does not
    /// try to "catch up" by rendering a long burst of frames as fast as it
    /// can.
    ///
    /// Returns the number of frames that were skipped. `threshold` must be at
    /// least `1`; values below `1` are treated as `1`. `max_skip` upper-bounds
    /// the skip to defend against absurd clock readings (e.g. the laptop was
    /// suspended for hours).
    pub fn catch_up(&mut self, threshold: u64, max_skip: u64) -> u64 {
        let threshold = threshold.max(1);
        let now = self.clock.now();
        // How many frames *should* have been rendered by `now`, computed
        // purely from the pacer's notion of `start`. This is monotonic in
        // `now` so we can compare directly against `frame_index`.
        let elapsed = now.saturating_duration_since(self.start);
        let expected = (elapsed.as_secs_f64() / self.frame_period_secs) as u64;
        if expected <= self.frame_index + threshold {
            return 0;
        }
        let behind = expected - self.frame_index;
        let skip = behind.min(max_skip);
        self.frame_index = self.frame_index.saturating_add(skip);
        skip
    }
}
