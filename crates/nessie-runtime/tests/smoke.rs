//! End-to-end smoke test for the **full runtime path**, mirroring
//! `nessie-core`'s `tests/smoke.rs` but going through [`nessie_runtime::Session`]
//! with mock [`AudioSink`] / [`FrameSink`] implementations.
//!
//! Spec §8.3 calls for both a core-only and a runtime-level smoke gate so a
//! regression in either layer (engine *or* the threading / sink plumbing) is
//! caught by `cargo test --workspace` on every CI matrix entry.
//!
//! What the test asserts:
//!
//! 1. The `Session` thread runs exactly 120 frames against the homebrew
//!    `smoke.nes` fixture and exits cleanly.
//! 2. The final framebuffer (captured by the mock `FrameSink` from the last
//!    `submit` call) hashes to the same SHA-1 as the core-only smoke test —
//!    i.e. the runtime layer adds no rendering side effects.
//! 3. The mock `AudioSink` received a non-zero number of `f32` samples.
//! 4. Frame indices are strictly increasing `0..120`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nessie_core::Nes;
use nessie_runtime::{AudioSink, FrameSink, Session, SessionOptions, SystemClock, FRAME_BYTES};
use parking_lot::Mutex;
use sha1::{Digest, Sha1};

/// SHA-1 of `[0u8; 256 * 240 * 4]` — i.e. an entirely black RGBA framebuffer.
/// The smoke ROM never enables rendering, so the PPU framebuffer is exactly
/// this value at the end of frame 120. Identical to the digest used by
/// `nessie-core/tests/smoke.rs`; sharing the constant is intentional so any
/// future change to the fixture has to be reflected in both tests.
const EXPECTED_FRAME_SHA1: &str = "64646545171a7f81dfcd027089d1f38a7f81b82b";

/// Locate the shared `smoke.nes` fixture relative to this crate's manifest
/// dir. The runtime crate does not own a copy of the fixture — it borrows
/// the one that lives next to `nessie-core` so there is exactly one source
/// of truth for the smoke ROM bytes.
fn smoke_rom_path() -> PathBuf {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest
        .parent()
        .expect("CARGO_MANIFEST_DIR has a parent (workspace `crates/`)")
        .join("nessie-core")
        .join("tests")
        .join("fixtures")
        .join("smoke.nes")
}

fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    let d = hasher.finalize();
    let mut s = String::with_capacity(40);
    for b in d {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

struct MockAudio {
    samples: AtomicUsize,
    rate: u32,
}

impl AudioSink for MockAudio {
    fn push_samples(&self, samples: &[f32]) {
        self.samples.fetch_add(samples.len(), Ordering::Relaxed);
    }
    fn sample_rate(&self) -> u32 {
        self.rate
    }
}

/// Captures every submitted frame index plus a copy of the **last**
/// framebuffer. Copying only the last frame keeps the test memory-light
/// while still letting us assert on the final pixel state.
struct MockFrames {
    count: AtomicUsize,
    indices: Mutex<Vec<u64>>,
    last_frame: Mutex<Vec<u8>>,
}

impl FrameSink for MockFrames {
    fn submit(&self, frame: &[u8; FRAME_BYTES], frame_index: u64) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.indices.lock().push(frame_index);
        let mut slot = self.last_frame.lock();
        slot.clear();
        slot.extend_from_slice(frame);
    }
}

#[test]
fn runtime_smoke_rom_runs_120_frames_via_session() {
    let bytes = std::fs::read(smoke_rom_path()).expect("smoke.nes fixture must exist");
    let nes = Nes::from_ines(&bytes).expect("smoke.nes must parse");
    let info = nes.cartridge_info();
    assert_eq!(info.mapper, 0, "smoke.nes is NROM (mapper 0)");
    assert!(!info.has_battery, "smoke.nes is not a battery cartridge");

    let audio = Arc::new(MockAudio {
        samples: AtomicUsize::new(0),
        rate: 44_100,
    });
    let frames = Arc::new(MockFrames {
        count: AtomicUsize::new(0),
        indices: Mutex::new(Vec::with_capacity(120)),
        last_frame: Mutex::new(Vec::with_capacity(FRAME_BYTES)),
    });

    // `paced: false` makes the test deterministic and fast; the threading
    // model and sink plumbing are exercised regardless of pacing.
    let opts = SessionOptions {
        paced: false,
        stop_after_frames: Some(120),
        ..SessionOptions::default()
    };

    let mut session = Session::start_with(
        nes,
        Arc::clone(&audio),
        Arc::clone(&frames),
        opts,
        SystemClock,
    );
    session.wait().expect("session thread should not panic");

    // Frame count + monotonic indices.
    assert_eq!(
        frames.count.load(Ordering::Relaxed),
        120,
        "FrameSink should have received exactly 120 submissions"
    );
    let indices = frames.indices.lock();
    assert_eq!(indices.first().copied(), Some(0));
    assert_eq!(indices.last().copied(), Some(119));
    for w in indices.windows(2) {
        assert!(w[0] < w[1], "frame indices must be strictly increasing");
    }
    drop(indices);

    // The runtime layer must not perturb the framebuffer hash.
    let last = frames.last_frame.lock();
    assert_eq!(
        last.len(),
        FRAME_BYTES,
        "captured framebuffer is full RGBA8 256×240"
    );
    let actual = sha1_hex(&last);
    assert_eq!(
        actual, EXPECTED_FRAME_SHA1,
        "runtime-submitted framebuffer SHA-1 must match the core smoke test"
    );

    // APU must have produced audible audio through the sink.
    assert!(
        audio.samples.load(Ordering::Relaxed) > 0,
        "AudioSink should have received non-zero samples"
    );
}
