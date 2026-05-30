//! `nessie-runtime` is the host-agnostic glue that drives [`nessie_core::Nes`]
//! on a background thread, owns the frame pacer, and exposes [`AudioSink`] /
//! [`FrameSink`] traits for hosts (cpal + Tauri channel in the desktop app).
//!
//! See spec §2 and §6.1: the runtime is the thin layer between the engine-only
//! `nessie-core` and the platform-specific shell. By keeping the
//! audio/frame/timing concerns behind traits we can:
//!
//! - swap a real [`cpal`-backed sink for a mock in tests](crate::sink),
//! - drive the pacer with a fake [`Clock`] so timing tests are deterministic,
//! - and reuse the same loop for headless integration tests, future WASM
//!   targets, or a CLI conformance runner without dragging in any GUI code.
//!
//! The crate has **no Tauri, no cpal, no winit dependencies** — those live in
//! `app/src-tauri`. Everything here is pure Rust + a tiny set of well-known
//! crates ([`parking_lot`], [`crossbeam_queue`], [`log`]).

mod clock;
mod pacer;
mod session;
mod sink;

pub use clock::{Clock, SystemClock};
pub use pacer::{FramePacer, NTSC_FPS};
pub use session::{BatteryOut, Session, SessionOptions};
pub use sink::{AudioSink, FrameSink, FRAME_BYTES};

/// Re-exports of the `nessie-core` button / player enums so hosts do not have
/// to depend on `nessie-core` directly when wiring up input forwarding through
/// a [`Session`].
pub use nessie_core::{Button, Player};
