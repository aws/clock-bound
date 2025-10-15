//! Clocks used in ClockBound
use crate::daemon::time::{Instant, inner::Clock, instant::Utc};
use nix::time::{ClockId, clock_gettime};

/// Wrapper around reads of the internal clock tracked by the ClockBound `ClockSyncAlgorithm`.
pub struct ClockBound;
impl Clock<Utc> for ClockBound {
    /// Get the current `Instant` by reading `ClockParameters`
    fn get_time(&self) -> Instant {
        todo!("implement this clock once we have some sane parameters");
    }
}

/// Wrapper around `CLOCK_REALTIME` reads, which provides a UTC timestamp.
/// `CLOCK_REALTIME` is steered by userspace clock corrections of phase and frequency (e.g. that's our job),
/// and can jump forwards and backwards.
pub struct RealTime;
impl Clock<Utc> for RealTime {
    /// Get the current `Instant` by reading `CLOCK_REALTIME`
    ///
    /// # Panics
    /// Panics if `clock_gettime` call fails (if pointer allocated for the call is invalid, or `ClockId` supplied is invalid or unavailable on the system)
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clock_gettime tv_nsec should be between 0 and 1e9-1 so no truncation"
    )]
    #[allow(
        clippy::cast_sign_loss,
        reason = "clock_gettime tv_nsec should be between 0 and 1e9-1 so no loss of sign"
    )]
    fn get_time(&self) -> Instant {
        // Unwrap safety: `nix` crate supplies valid pointer and `ClockId` so the `clock_gettime` call should not be able to fail
        let now = clock_gettime(ClockId::CLOCK_REALTIME).unwrap();
        Instant::from_time(now.tv_sec().into(), now.tv_nsec() as u32)
    }
}

/// Wrapper around `CLOCK_MONOTONIC_RAW` reads, which provides a UTC timestamp.
///
/// `CLOCK_MONOTONIC_RAW` is controlled solely in the kernel, unaffected by phase and frequency corrections.
/// It simply has its rate of change aligned to that specified by the arch counter frequency hardware spec, so it
/// may be slow or fast depending on the state of the underlying oscillator.
pub struct MonotonicRaw;
impl Clock<Utc> for MonotonicRaw {
    /// Get the current `Instant` by reading `CLOCK_MONOTONIC_RAW`
    ///
    /// # Panics
    /// Panics if `clock_gettime` call fails (if pointer allocated for the call is invalid, or `ClockId` supplied is invalid or unavailable on the system)
    #[allow(
        clippy::cast_possible_truncation,
        reason = "clock_gettime tv_nsec should be between 0 and 1e9-1 so no truncation"
    )]
    #[allow(
        clippy::cast_sign_loss,
        reason = "clock_gettime tv_nsec should be between 0 and 1e9-1 so no loss of sign"
    )]
    fn get_time(&self) -> Instant {
        // Unwrap safety: `nix` crate supplies valid pointer and `ClockId` so the `clock_gettime` call should not be able to fail
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC_RAW).unwrap();
        Instant::from_time(now.tv_sec().into(), now.tv_nsec() as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ensure that CLOCK_REALTIME and the std library utility for CLOCK_REALTIME are approximately the same.
    #[test]
    fn test_realtime() {
        let realtime = RealTime;
        let now = realtime.get_time();
        let now_std = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        assert!(now_std.as_secs().abs_diff(now.as_seconds() as u64) < 1);
    }

    // Naively show that CLOCK_MONOTONIC_RAW is monotonic (and hope that it's raw)
    #[test]
    fn test_monotonic_raw() {
        let monotonic_raw = MonotonicRaw;
        let now = monotonic_raw.get_time();
        let later = monotonic_raw.get_time();
        assert!(now < later);
    }
}
