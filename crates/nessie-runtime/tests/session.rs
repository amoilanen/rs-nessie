//! Integration test: spin up a [`Session`] on the public-domain `smoke.nes`
//! fixture, run 60 frames through the runtime, and assert that the mock
//! sinks recorded 60 frame submissions and a non-zero number of audio
//! samples.
//!
//! Mirrors the spec §8.3 "full runtime path" check the workflow plan calls
//! out for this step.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use nessie_core::Nes;
use nessie_runtime::{AudioSink, FrameSink, Session, SessionOptions, SystemClock, FRAME_BYTES};
use parking_lot::Mutex;

/// `smoke.nes` lives in `nessie-core`'s fixtures directory; we read it via
/// `CARGO_MANIFEST_DIR` (resolved at build time for the *runtime* crate) so
/// the test does not depend on any working directory.
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

struct MockFrames {
    count: AtomicUsize,
    indices: Mutex<Vec<u64>>,
}

impl FrameSink for MockFrames {
    fn submit(&self, frame: &[u8; FRAME_BYTES], frame_index: u64) {
        assert_eq!(
            frame.len(),
            FRAME_BYTES,
            "frame must be the full RGBA8 buffer"
        );
        self.count.fetch_add(1, Ordering::Relaxed);
        self.indices.lock().push(frame_index);
    }
}

#[test]
fn session_runs_smoke_rom_for_60_frames() {
    let bytes = std::fs::read(smoke_rom_path()).expect("smoke.nes fixture must exist");
    let nes = Nes::from_ines(&bytes).expect("smoke.nes must parse");

    let audio = Arc::new(MockAudio {
        samples: AtomicUsize::new(0),
        rate: 44_100,
    });
    let frames = Arc::new(MockFrames {
        count: AtomicUsize::new(0),
        indices: Mutex::new(Vec::with_capacity(60)),
    });

    let opts = SessionOptions {
        paced: false,
        stop_after_frames: Some(60),
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

    assert_eq!(
        frames.count.load(Ordering::Relaxed),
        60,
        "FrameSink should have received exactly 60 submissions"
    );
    let indices = frames.indices.lock();
    assert_eq!(indices.first().copied(), Some(0));
    assert_eq!(indices.last().copied(), Some(59));
    // Indices must be strictly increasing.
    for w in indices.windows(2) {
        assert!(w[0] < w[1], "frame indices must be strictly increasing");
    }
    drop(indices);

    assert!(
        audio.samples.load(Ordering::Relaxed) > 0,
        "AudioSink should have received non-zero samples"
    );
}
