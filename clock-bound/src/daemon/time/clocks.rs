//! Clocks used in ClockBound
use crate::daemon::{
    clock_parameters::ClockParameters,
    io::tsc::ReadTsc,
    time::{Instant, TscCount, inner::Clock, instant::Utc},
};
use nix::time::{ClockId, clock_gettime};

/// Wrapper around reads of the internal clock tracked by the ClockBound `ClockSyncAlgorithm`.
pub struct ClockBound<T> {
    clock_parameters: ClockParameters,
    read_tsc: T,
}
impl<T> ClockBound<T> {
    /// Create a new `ClockBound` clock
    pub fn new(clock_parameters: ClockParameters, read_tsc: T) -> Self {
        Self {
            clock_parameters,
            read_tsc,
        }
    }
}

impl<T: ReadTsc> Clock<Utc> for ClockBound<T> {
    /// Get the current `Instant` by reading `ClockParameters`
    fn get_time(&self) -> Instant {
        let current_tsc = TscCount::new(self.read_tsc.read_tsc().into());
        // TODO: disregarding overflow of TSC, that will take a while, but worth thinking of
        self.clock_parameters.time
            + ((current_tsc - self.clock_parameters.tsc_count) * self.clock_parameters.period)
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
    fn get_time(&self) -> Instant {
        // Unwrap safety: `nix` crate supplies valid pointer and `ClockId` so the `clock_gettime` call should not be able to fail
        let now = clock_gettime(ClockId::CLOCK_REALTIME).unwrap();
        Instant::from_timespec(now)
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
    fn get_time(&self) -> Instant {
        // Unwrap safety: `nix` crate supplies valid pointer and `ClockId` so the `clock_gettime` call should not be able to fail
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC_RAW).unwrap();
        Instant::from_timespec(now)
    }
}

pub struct MonotonicCoarse;
impl Clock<Utc> for MonotonicCoarse {
    /// Get the current `Instant` by reading `CLOCK_MONOTONIC_COARSE`
    ///
    /// # Panics
    /// Panics if `clock_gettime` call fails (if pointer allocated for the call is invalid, or `ClockId` supplied is invalid or unavailable on the system)
    fn get_time(&self) -> Instant {
        // Unwrap safety: `nix` crate supplies valid pointer and `ClockId` so the `clock_gettime` call should not be able to fail
        let now = clock_gettime(ClockId::CLOCK_MONOTONIC_COARSE).unwrap();
        Instant::from_timespec(now)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::daemon::{
        io::tsc::MockReadTsc,
        time::{Duration, tsc::Period},
    };

    use super::*;

    // Ensure that CLOCK_REALTIME and the std library utility for CLOCK_REALTIME are approximately the same.
    #[test]
    fn test_realtime() {
        let realtime = RealTime;
        let now = realtime.get_time();
        let now_std = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap();
        let now = now - Instant::UNIX_EPOCH;
        approx::assert_abs_diff_eq!(now_std.as_secs_f64(), now.as_seconds_f64(), epsilon = 0.01)
    }

    // Naively show that CLOCK_MONOTONIC_RAW is monotonic (and hope that it's raw)
    #[test]
    fn test_monotonic_raw() {
        let monotonic_raw = MonotonicRaw;
        let now = monotonic_raw.get_time();
        let later = monotonic_raw.get_time();
        assert!(now < later);
    }

    #[rstest]
    #[case::no_tsc_change(
        TscCount::new(0),
        Instant::from_secs(0),
        Period::from_seconds(1e-9),
        0,
        Instant::from_secs(0)
    )]
    #[case::start_from_zero_time(
        TscCount::new(0),
        Instant::from_secs(0),
        Period::from_seconds(1e-9),
        1_000_000_000,
        Instant::from_secs(1)
    )]
    #[case::start_from_nonzero_time(
        TscCount::new(0),
        Instant::from_secs(1),
        Period::from_seconds(1e-9),
        1_000_000_000,
        Instant::from_secs(2)
    )]
    #[case::larger_period(
        TscCount::new(0),
        Instant::from_secs(0),
        Period::from_seconds(1e-6),
        1_000_000_000,
        Instant::from_secs(1_000)
    )]
    #[case::start_from_nonzero_tsc(
        TscCount::new(1_000_000_000),
        Instant::from_secs(0),
        Period::from_seconds(1e-9),
        2_000_000_000,
        Instant::from_secs(1)
    )]
    fn test_clockbound_clock(
        #[case] initial_tsc: TscCount,
        #[case] initial_time: Instant,
        #[case] period: Period,
        #[case] read_tsc_output: u64,
        #[case] expected_time: Instant,
    ) {
        let mut mock_read_tsc = MockReadTsc::new();
        mock_read_tsc
            .expect_read_tsc()
            .returning(move || read_tsc_output);

        let tsc_count = initial_tsc;
        let time = initial_time;
        let clock_error_bound = Duration::new(0);
        let period_max_error = Period::from_seconds(0.0);
        let as_of_monotonic = Instant::from_secs(2);
        let clock_parameters = ClockParameters {
            tsc_count,
            time,
            clock_error_bound,
            period,
            period_max_error,
            as_of_monotonic,
        };
        let clockbound_clock = ClockBound::new(clock_parameters, mock_read_tsc);
        assert_eq!(clockbound_clock.get_time(), expected_time);
    }
}
