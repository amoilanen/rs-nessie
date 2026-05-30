//! Unit tests for [`FramePacer`].
//!
//! Drive the pacer from a [`FakeClock`] so timing is deterministic — no real
//! sleeps, no flakiness on busy CI runners.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{FramePacer, NTSC_FPS};
use crate::clock::Clock;

/// Test-only clock the test drives forward in fixed steps.
struct FakeClock {
    now: Mutex<Instant>,
}

impl FakeClock {
    fn new(start: Instant) -> Self {
        Self {
            now: Mutex::new(start),
        }
    }

    fn advance(&self, by: Duration) {
        let mut g = self.now.lock().unwrap();
        *g += by;
    }

    fn set(&self, t: Instant) {
        *self.now.lock().unwrap() = t;
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        *self.now.lock().unwrap()
    }
}

fn frame_period() -> Duration {
    Duration::from_secs_f64(1.0 / NTSC_FPS)
}

#[test]
fn deadlines_are_evenly_spaced_at_the_ntsc_period() {
    let t0 = Instant::now();
    let pacer = FramePacer::new(FakeClock::new(t0), 44_100);
    let period = frame_period();
    // Allow a microsecond of slack to absorb f64 → Duration rounding.
    let slack = Duration::from_micros(1);

    for i in 0..10 {
        let expected = t0 + period * i;
        let actual = pacer.deadline_for(u64::from(i));
        let diff = if actual > expected {
            actual - expected
        } else {
            expected - actual
        };
        assert!(diff <= slack, "frame {i}: deadline drifted by {diff:?}",);
    }
}

#[test]
fn wait_for_next_frame_is_full_period_initially() {
    let t0 = Instant::now();
    let pacer = FramePacer::new(FakeClock::new(t0), 44_100);
    let wait = pacer.wait_for_next_frame();
    let period = frame_period();
    let diff = if wait > period {
        wait - period
    } else {
        period - wait
    };
    assert!(diff < Duration::from_micros(2));
}

#[test]
fn wait_returns_zero_when_already_past_the_deadline() {
    let t0 = Instant::now();
    let clock = FakeClock::new(t0);
    let pacer = FramePacer::new(&clock, 44_100);
    // Advance past two frames; the next-frame deadline is now in the past.
    clock.advance(frame_period() * 3);
    assert_eq!(pacer.wait_for_next_frame(), Duration::ZERO);
}

#[test]
fn advance_progresses_the_frame_index() {
    let pacer_clock = FakeClock::new(Instant::now());
    let mut pacer = FramePacer::new(pacer_clock, 44_100);
    assert_eq!(pacer.frame_index(), 0);
    assert_eq!(pacer.advance(), 1);
    assert_eq!(pacer.advance(), 2);
    assert_eq!(pacer.frame_index(), 2);
}

#[test]
fn issues_frames_at_expected_cadence_under_a_fake_clock() {
    // Render 60 frames while advancing the fake clock by one frame period
    // each iteration. Each `wait_for_next_frame` should be approximately one
    // frame period (the clock advance happens *after* the wait is observed).
    let t0 = Instant::now();
    let clock = FakeClock::new(t0);
    let mut pacer = FramePacer::new(&clock, 44_100);
    let period = frame_period();

    let mut waits = Vec::with_capacity(60);
    for _ in 0..60 {
        waits.push(pacer.wait_for_next_frame());
        pacer.advance();
        clock.advance(period);
    }

    // All waits should be very close to `period`. Allow ±2 microseconds for
    // f64 rounding.
    for (i, w) in waits.iter().enumerate() {
        let diff = if *w > period {
            *w - period
        } else {
            period - *w
        };
        assert!(
            diff < Duration::from_micros(2),
            "wait[{i}] = {w:?} drifted from {period:?}",
        );
    }

    // After 60 frames the pacer's notion of "next deadline" should be very
    // close to `t0 + 61 * period`.
    let expected = t0 + period * 61;
    let actual = pacer.deadline_for(61);
    let diff = if actual > expected {
        actual - expected
    } else {
        expected - actual
    };
    assert!(diff < Duration::from_micros(2));
}

#[test]
fn catch_up_skips_after_a_stall_without_over_running() {
    // Simulate a 1 s stall (≈ 60 frames) after only 1 frame was rendered.
    let t0 = Instant::now();
    let clock = FakeClock::new(t0);
    let mut pacer = FramePacer::new(&clock, 44_100);

    let period = frame_period();
    // Render one frame normally.
    clock.advance(period);
    pacer.advance();

    // Now stall for ~1 second of wall-clock time — way past the next
    // deadline.
    clock.advance(Duration::from_secs_f64(1.0));

    // Threshold = 2 means "if more than 2 frames behind, skip". Max skip
    // bounded to 1000 to defend against pathological clock jumps.
    let skipped = pacer.catch_up(2, 1_000);
    // We were 60 frames behind, so we expect roughly 60 skipped.
    assert!(
        (58..=62).contains(&skipped),
        "expected ≈60 skipped frames after 1 s stall, got {skipped}",
    );

    // After the catch-up, the pacer's next-frame wait should be small
    // (under one period), proving we did **not** over-run by emitting every
    // skipped frame.
    let wait = pacer.wait_for_next_frame();
    assert!(
        wait <= period,
        "pacer over-ran after catch-up: wait={wait:?}",
    );
}

#[test]
fn catch_up_is_a_noop_when_inside_threshold() {
    let t0 = Instant::now();
    let clock = FakeClock::new(t0);
    let mut pacer = FramePacer::new(&clock, 44_100);
    // Advance one frame worth of time but render zero frames — well within
    // a threshold of 5.
    clock.advance(frame_period());
    let skipped = pacer.catch_up(5, 1_000);
    assert_eq!(skipped, 0);
    assert_eq!(pacer.frame_index(), 0);
}

#[test]
fn catch_up_respects_max_skip_bound() {
    let t0 = Instant::now();
    let clock = FakeClock::new(t0);
    let mut pacer = FramePacer::new(&clock, 44_100);
    // Jump the clock forward by a full hour: ~216_000 frames behind.
    clock.set(t0 + Duration::from_secs(3_600));
    let skipped = pacer.catch_up(1, 100);
    assert_eq!(skipped, 100, "max_skip must cap the catch-up step");
    assert_eq!(pacer.frame_index(), 100);
}

#[test]
#[should_panic(expected = "sample_rate must be positive")]
fn new_panics_on_zero_sample_rate() {
    let _ = FramePacer::new(FakeClock::new(Instant::now()), 0);
}
