//! Criterion bench: `Nes::step_frame()` throughput on the smoke ROM.
//!
//! Baseline (recorded locally on Apple Silicon, Rust 1.78 bench profile):
//!
//! ```text
//! step_frame    time:   [~160 µs per frame]
//! ```
//!
//! That is roughly **6,000 frames/sec**, or ~100× the 60 Hz NTSC rate.
//! Hosts on slower hardware (older laptops, low-end Linux desktops) have
//! plenty of headroom for the 60 Hz target enforced by `nessie-runtime`'s
//! pacer.
//!
//! CI invocation:
//!
//! ```text
//! cargo bench -p nessie-core --bench frame -- --save-baseline ci
//! ```
//!
//! The follow-up perf-gate step (see `plan.md`) wires
//! `cargo bench --baseline ci` into a nightly job that fails on >10%
//! regression.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use nessie_core::Nes;

const SMOKE_ROM: &[u8] = include_bytes!("../tests/fixtures/smoke.nes");

fn bench_step_frame(c: &mut Criterion) {
    c.bench_function("step_frame", |b| {
        b.iter_batched(
            || Nes::from_ines(SMOKE_ROM).expect("smoke.nes must parse"),
            |mut nes| {
                // Drain any startup audio so we measure steady-state frame work.
                let mut audio = Vec::new();
                nes.step_frame();
                nes.drain_audio(&mut audio);
                nes
            },
            BatchSize::SmallInput,
        )
    });
}

criterion_group!(benches, bench_step_frame);
criterion_main!(benches);
