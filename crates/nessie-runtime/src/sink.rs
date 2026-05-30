//! Sink traits for audio samples and rendered frames.
//!
//! These are the only two host-facing extension points the runtime exposes.
//! Implementations live in the Tauri shell ([`cpal`]-backed [`AudioSink`] and
//! a Tauri-channel-backed [`FrameSink`]); tests substitute mocks.
//!
//! Both traits take `&self` so implementations can be shared across threads
//! via `Arc`: the emulation thread pushes audio / submits frames while the
//! main thread may read counters or queue depths for diagnostics.

/// Size in bytes of one NES framebuffer (256 × 240 RGBA8).
///
/// Re-exposed from the runtime so hosts do not have to depend on
/// `nessie-core`'s `ppu::FRAMEBUFFER_BYTES` directly.
pub const FRAME_BYTES: usize = 256 * 240 * 4;

/// Consumer of audio samples produced by the emulation thread.
///
/// The runtime calls [`push_samples`](AudioSink::push_samples) every frame
/// with however many samples the APU produced at the rate returned by
/// [`sample_rate`](AudioSink::sample_rate). Implementations are expected to
/// be lock-free / wait-free in the audio callback path: dropping the oldest
/// samples on overrun is preferred over blocking the emulation thread (see
/// spec §6.1).
pub trait AudioSink: Send + Sync {
    /// Push a slice of mono `f32` samples into the sink. The slice's length
    /// is whatever the APU drained for the most recent frame. Implementations
    /// must never block the caller for more than a few microseconds.
    fn push_samples(&self, samples: &[f32]);

    /// The sample rate (in Hz) the sink expects. The runtime forwards this to
    /// the [`FramePacer`](crate::FramePacer) so video pacing follows audio.
    fn sample_rate(&self) -> u32;
}

/// Consumer of rendered framebuffers produced by the emulation thread.
///
/// The runtime calls [`submit`](FrameSink::submit) once per emulated NES
/// frame with the contents of [`nessie_core::Nes::framebuffer`]. The
/// `frame_index` is monotonically increasing starting at 0 for the first
/// frame submitted by the session.
pub trait FrameSink: Send + Sync {
    /// Submit a fresh 256×240 RGBA8 framebuffer. The reference is only valid
    /// for the duration of the call; implementations that need to keep the
    /// pixels must copy them (e.g. into a Tauri channel message buffer).
    fn submit(&self, frame: &[u8; FRAME_BYTES], frame_index: u64);
}
