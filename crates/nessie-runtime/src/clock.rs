//! Time abstraction.
//!
//! Wrapping [`std::time::Instant::now`] behind a tiny [`Clock`] trait lets us
//! drive the [`FramePacer`](crate::FramePacer) and [`Session`](crate::Session)
//! from a fake clock in unit tests without sleeping. The desktop host uses
//! [`SystemClock`] which is a zero-cost wrapper around `Instant::now()`.

use std::time::Instant;

/// Abstracts wall-clock time so tests can advance time without sleeping.
///
/// Implementors must be both [`Send`] and [`Sync`] because the same clock is
/// shared between the emulation thread and the host's main thread inside a
/// [`Session`](crate::Session).
pub trait Clock: Send + Sync {
    /// Current monotonically-increasing instant.
    fn now(&self) -> Instant;
}

/// Default clock that delegates to [`Instant::now`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    #[inline]
    fn now(&self) -> Instant {
        Instant::now()
    }
}

// Allow `&C` to satisfy `Clock` so callers that already own a clock can pass
// a reference without giving up ownership. Helpful when the same clock is
// shared between the pacer and the surrounding session.
impl<C: Clock + ?Sized> Clock for &C {
    #[inline]
    fn now(&self) -> Instant {
        (**self).now()
    }
}
