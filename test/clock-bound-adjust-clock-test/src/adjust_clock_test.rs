//! Our test intends to validate that the clock steering of our `ClockAdjust` component is within our expectations, solely based
//! on the parameters passed to the kernel via `ClockAdjust`. We can improve on this in later iterations, by using actual `ClockParameters`
//! along with the TSC values/frequency we use for our baseline.
//!
//! After the clock parameters of `CLOCK_REALTIME` are reset, we mutate them again via `ClockAdjust` to steer the clock.
//!
//! The test then continually calculates the offset of `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW`, ensuring that this offset
//! is steered in the direction intended by the clock adjustment.
//! We should be able to estimate the offset of these two clocks at some given time based on our parameters, by adding the components
//! of the clock offset from phase correction and skew parameters supplied.
//!
//! =========================================================================================================
//!                                           PHASE CORRECTION
//! =========================================================================================================
//! Phase correction follows an exponentially decaying curve.
//!
//! This graph is the offset of `CLOCK_REALTIME` from `CLOCK_MONOTONIC_RAW`, with `CLOCK_REALTIME` starting 500ms ahead
//! and being given a -500ms phase correction.
//!
//! Offset (s), with `CLOCK_REALTIME` starting 500ms ahead and being handed a -500ms phase correction.
//!      ^
//! 0.50 | *
//!      |
//! 0.45 |
//!      |
//! 0.40 |
//!      |           *
//! 0.35 |
//!      |
//! 0.30 |
//!      |                         *
//! 0.25 |                         
//!      |
//! 0.20 |                                       *
//!      |
//! 0.15 |                                                   *
//!      |
//! 0.10 |                                                                 *
//!      |
//! 0.05 |                                                                              *
//!      |
//! 0.00 |
//!      +------------+-------------|------------|------------|------------|------------|------------> Time (s)
//!      0            1             2            3            4            5            6
//!
//!
//! =========================================================================================================
//!                                           SKEW/FREQUENCY CORRECTION
//! =========================================================================================================
//! A skew/frequency correction applied to the clock is a linear component, and simply changes the slope of the clock correction.
//!
//! This graph is the offset of `CLOCK_REALTIME` from `CLOCK_MONOTONIC_RAW`, with `CLOCK_REALTIME` starting aligned, and being given a
//! +0.05s skew correction (this is invalid in kernel but showing for demo's sake)
//!
//! Offset (s)
//!      ^
//! 0.50 |
//!      |
//! 0.45 |
//!      |
//! 0.40 |
//!      |
//! 0.35 |
//!      |
//! 0.30 |                                                                              *
//!      |
//! 0.25 |                                                                 *
//!      |
//! 0.20 |                                                    *
//!      |
//! 0.15 |                                       *
//!      |
//! 0.10 |                          *
//!      |
//! 0.05 |            *
//!      |
//! 0.00 |*
//!      +------------+-------------|------------|------------|------------|------------|------------> Time (s)
//!      0            1             2            3            4            5            6
//! This is a test which validates that our `ClockAdjust` functions work as expected, applying the
//! phase correction and skew correction parameters as we expect to via calls to that API, and seeing
//! the `CLOCK_REALTIME` adjustment is steered precisely to where we would expect it to be.
#![allow(clippy::doc_comment_double_space_linebreaks, reason = "hooray ascii")]
use clock_bound::daemon::{
    clock_state::clock_adjust::{KAPIClockAdjuster, NtpAdjTimeError, NtpAdjTimeExt},
    time::{Duration, Instant, tsc::Skew},
};
use tracing::info;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    tracing_subscriber::fmt().init();
    todo!("implement me");
}

#[derive(Debug)]
#[allow(dead_code)]
struct ClockAdjustTestParameters {
    /// Frequency correction (skew parameter) passed into `ClockAdjust`
    skew: Skew,
    /// Phase correction passed into `ClockAdjust`
    phase_correction: Duration,
    /// Start time of the test/clock adjustment
    /// We need this to extrapolate the change in clock offsets
    /// at some future `Instant`. Notably, since this is calculated
    /// separately from when the kernel ACTUALLY processes our clock adjustment,
    /// there is some inherent error. It may be possible to reduce that error if we
    /// grab this `start_time` from the `ClockAdjust`'s underlying `adjtimex` output `time`
    /// instead.
    start_time: Instant,
    /// Initial offset of the `CLOCK_REALTIME` and `CLOCK_MONOTONIC` at
    /// test start. We need this to extrapolate the change in clock offsets
    /// at some future `Instant`.
    start_offset: Duration,
}

#[allow(dead_code)]
impl ClockAdjustTestParameters {
    /// Return `ClockAdjustTestParameters`, which can be
    /// used to calculate a future change in the `CLOCK_REALTIME` change in phase offset
    /// with respect to `CLOCK_MONOTONIC_RAW`.
    fn new(
        skew: Skew,
        phase_correction: Duration,
        start_offset: Duration,
        start_time: Instant,
    ) -> Self {
        Self {
            skew,
            phase_correction,
            start_time,
            start_offset,
        }
    }

    /// Get the start time of the test, which is needed in order to
    /// estimate how much the clock has been adjusted at some future `Instant`.
    fn get_start_time(&self) -> Instant {
        self.start_time
    }

    /// Get the starting offset of the `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW`
    /// for the test, which is needed in order to estimate how much the clock has
    /// been adjusted at some future `Instant`.
    ///
    /// The starting offset should be taken at a point where `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW`
    /// are both running with equivalent frequency and no active slew, so they should be ticking
    /// at the same rate.
    ///
    /// The caller can then compare this initial offset with an offset at some point in the future, and
    /// the difference represents how much the clock has been steered by `ClockAdjust`.
    fn get_start_offset(&self) -> Duration {
        self.start_offset
    }

    /// The total expected change in the offset between `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW` after
    /// a clock adjustment has begun.
    fn expected_offset_change_at(&self, time: Instant) -> Duration {
        let phase_offset_change = self.expected_offset_change_due_to_phase_correction_at(time);
        let skew_offset_change = self.expected_offset_change_due_to_skew_at(time);
        phase_offset_change + skew_offset_change
    }

    /// Returns the expected change in the offset between `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW` based on the
    /// `phase_correction` factor.
    ///
    /// Linux kernel PLL correction begins at the top of a second (once fractional part of second overflows)
    /// It calculates a PLL correction to make over the next second, and does so repeatedly.
    /// The calculation of the slew the kernel uses at some point is done via a bitshift of the
    /// current remaining offset.
    #[allow(
        clippy::cast_precision_loss,
        reason = "`current_pll_correction_nanos` is not expected to be large enough to cause precision loss"
    )]
    fn expected_offset_change_due_to_phase_correction_at(&self, time: Instant) -> Duration {
        // PLL correction begins at top of a second - we calculate when this PLL start time `time_since_pll_start` was based on the start time,
        // and also calculate the `Duration` that has passed between `time` and `time_since_pll_start`.
        let start_time_fractional_nanos = self.get_start_time().as_nanos() % 1_000_000_000;
        let time_to_next_second = 1_000_000_000 - start_time_fractional_nanos;
        let pll_start_time = self.get_start_time() + Duration::from_nanos(time_to_next_second);
        let time_since_pll_start = time - pll_start_time;

        // Only work with positive values to make our lives easier. We'll deal with it after.
        let mut remaining_correction = if self.phase_correction.get() < 0 {
            -self.phase_correction
        } else {
            self.phase_correction
        };

        // Based on the initial `phase_correction` passed in, we emulate what the kernel is doing -
        // take a bitshifted portion of the `remaining_correction` and subtract that each second.
        let mut duration_of_correction = Duration::from_secs(0);
        while duration_of_correction < time_since_pll_start {
            // Bitshifted PLL correction, which is calculated in kernel as
            // `RemainingOffset` >> 2 + `TimeConstant` (another kernel clock adjustment parameter, which we use 0 for today).
            let current_pll_correction_nanos = remaining_correction.as_nanos() >> 2;
            let duration_of_current_pll_correction = time_since_pll_start - duration_of_correction;
            // Consume a full second of correction if we can,
            // else consume the fractional portion of the second remaining
            if time_since_pll_start - duration_of_correction >= Duration::from_secs(1) {
                remaining_correction -= Duration::from_nanos(current_pll_correction_nanos);
                duration_of_correction += Duration::from_secs(1);
            } else {
                let correction = (current_pll_correction_nanos as f64
                    * duration_of_current_pll_correction.as_seconds_f64())
                    / 1e9;
                remaining_correction -= Duration::from_seconds_f64(correction);
                break;
            }
        }

        // `remaining_correction` is positive. If initial `phase_correction` was negative, we should add it,
        // else subtract it.
        if self.phase_correction.get() < 0 {
            self.phase_correction + remaining_correction
        } else {
            self.phase_correction - remaining_correction
        }
    }

    /// Returns the expected change in the offset between `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW` based on the
    /// `skew` factor. This is much more straightforward - `freq` in the kernel is applied immediately, so we can
    /// just do a simple linear extrapolation of the expected time.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "fine to truncate when we're working at nanosecond granularity for Linux kernel"
    )]
    #[allow(
        clippy::cast_precision_loss,
        reason = "scale of `time_since_start` should be small enough that precision is negligible"
    )]
    fn expected_offset_change_due_to_skew_at(&self, time: Instant) -> Duration {
        let time_since_start = time - self.get_start_time();
        let time_in_nanos = time_since_start.as_nanos();
        // This may introduce some loss in precision, but over the duration of these tests
        // and because of the nature of the measurement of the offset of the clocks, this is OK
        let time_in_nanos_adjusted_for_skew = time_in_nanos as f64 * self.skew.get();
        Duration::from_nanos(time_in_nanos_adjusted_for_skew as i64)
    }
}

/// Reset the system clock, by clearing any `skew` and `phase_correction` running in the kernel.
/// Give some buffer time by sleeping after this too.
#[allow(dead_code)]
async fn reset_clock() -> Result<(), NtpAdjTimeError> {
    info!("Resetting clock parameters..");
    let clock_adjuster = KAPIClockAdjuster;
    // Reset the kernel NTP parameters.
    let phase_correction = Duration::from_millis(0);
    let skew = Skew::from_ppm(0.0);
    clock_adjuster.adjust_clock(phase_correction, skew)?;
    // Ensure any ongoing slews are done
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const NANOS_PER_SEC: i64 = 1_000_000_000;

    struct OffsetChangeSnapshot {
        pub time_since_start: Duration,
        pub offset_change: Duration,
    }

    #[rstest]
    #[case::zero_phase_correction(
        Duration::from_secs(0),
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(0), offset_change: Duration::from_secs(0) },
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(10), offset_change: Duration::from_secs(0) },
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(100), offset_change: Duration::from_secs(0) },
        ],
    )]
    #[case::big_positive_phase_correction(
        Duration::from_millis(500), // Correct 0.5 seconds
        Instant::from(NANOS_PER_SEC / 2), // Start mid second
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500), // Skip to next second
                offset_change: Duration::from_secs(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(1),
                offset_change: Duration::from_nanos(125_000_000), // 0.125 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(2),
                offset_change: Duration::from_nanos(218_750_000), // ~0.219 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(10),
                offset_change: Duration::from_nanos(471_843_242),// ~0.471 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(20),
                offset_change: Duration::from_nanos(498_414_392),// ~0.498 seconds corrected
            },
        ],
    )]
    #[case::big_negative_phase_correction(
        -Duration::from_millis(500), // Correct 0.5 seconds
        Instant::from(NANOS_PER_SEC / 2), // Start mid second
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500), // Skip to next second
                offset_change: Duration::from_secs(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(1),
                offset_change: -Duration::from_nanos(125_000_000), // 0.125 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(2),
                offset_change: -Duration::from_nanos(218_750_000), // ~0.219 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(10),
                offset_change: -Duration::from_nanos(471_843_242),// ~0.471 seconds corrected
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500) + Duration::from_secs(20),
                offset_change: -Duration::from_nanos(498_414_392),// ~0.498 seconds corrected
            },
        ],
    )]
    fn test_expected_offset_change_due_to_phase_correction_at(
        #[case] phase_correction: Duration,
        #[case] start_time: Instant,
        #[case] start_offset: Duration,
        #[case] expected_changes_in_offset: Vec<OffsetChangeSnapshot>,
    ) {
        let params = ClockAdjustTestParameters::new(
            Skew::from_ppm(0.0),
            phase_correction,
            start_offset,
            start_time,
        );

        for expected_change in expected_changes_in_offset {
            let time = params.get_start_time() + expected_change.time_since_start;
            let res = params.expected_offset_change_due_to_phase_correction_at(time);
            assert_eq!(res, expected_change.offset_change);
        }
    }

    #[rstest]
    #[case::zero_skew(
        Skew::from_ppm(0.0),
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(0), offset_change: Duration::from_secs(0) },
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(1), offset_change: Duration::from_secs(0) },
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(10), offset_change: Duration::from_secs(0) },
            OffsetChangeSnapshot { time_since_start: Duration::from_secs(100), offset_change: Duration::from_secs(0) },
        ],
    )]
    #[case::positive_skew_100_ppm(
        Skew::from_ppm(100.0), // 100 ppm positive skew
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(0),
                offset_change: Duration::from_secs(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(1),
                offset_change: Duration::from_nanos(100_000) // 1s * 100ppm = 100μs = 100,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(10),
                offset_change: Duration::from_nanos(1_000_000) // 10s * 100ppm = 1ms = 1,000,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(100),
                offset_change: Duration::from_nanos(10_000_000) // 100s * 100ppm = 10ms = 10,000,000ns
            },
        ],
    )]
    #[case::negative_skew_50_ppm(
        Skew::from_ppm(-50.0), // 50 ppm negative skew
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(0),
                offset_change: Duration::from_secs(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(1),
                offset_change: Duration::from_nanos(-50_000) // 1s * -50ppm = -50μs = -50,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(10),
                offset_change: Duration::from_nanos(-500_000) // 10s * -50ppm = -0.5ms = -500,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(100),
                offset_change: Duration::from_nanos(-5_000_000) // 100s * -50ppm = -5ms = -5,000,000ns
            },
        ],
    )]
    #[case::large_positive_skew_1000_ppm(
        Skew::from_ppm(1000.0), // 1000 ppm (0.1%) positive skew
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(0),
                offset_change: Duration::from_secs(0),
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(1),
                offset_change: Duration::from_nanos(1_000_000) // 1s * 1000ppm = 1ms = 1,000,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(60),
                offset_change: Duration::from_nanos(60_000_000) // 60s * 1000ppm = 60ms = 60,000,000ns
            },
        ],
    )]
    #[case::fractional_time_positive_skew(
        Skew::from_ppm(200.0), // 200 ppm positive skew
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(0),
                offset_change: Duration::from_millis(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(500), // 0.5 seconds
                offset_change: Duration::from_nanos(100_000) // 0.5s * 200ppm = 100μs = 100,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(1500), // 1.5 seconds
                offset_change: Duration::from_nanos(300_000) // 1.5s * 200ppm = 300μs = 300,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(2750), // 2.75 seconds
                offset_change: Duration::from_nanos(550_000) // 2.75s * 200ppm = 550μs = 550,000ns
            },
        ],
    )]
    #[case::very_small_skew(
        Skew::from_ppm(0.1), // Very small 0.1 ppm skew
        Instant::from(NANOS_PER_SEC / 2),
        Duration::from_secs(0),
        vec![
            OffsetChangeSnapshot {
                time_since_start: Duration::from_millis(0),
                offset_change: Duration::from_millis(0)
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(1),
                offset_change: Duration::from_nanos(100) // 1s * 0.1ppm = 0.1μs = 100ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(10),
                offset_change: Duration::from_nanos(1_000) // 10s * 0.1ppm = 1μs = 1,000ns
            },
            OffsetChangeSnapshot {
                time_since_start: Duration::from_secs(1000),
                offset_change: Duration::from_nanos(100_000) // 1000s * 0.1ppm = 100μs = 100,000ns
            },
        ],
    )]
    fn test_expected_offset_change_due_to_skew_at(
        #[case] skew: Skew,
        #[case] start_time: Instant,
        #[case] start_offset: Duration,
        #[case] expected_changes_in_offset: Vec<OffsetChangeSnapshot>,
    ) {
        let params = ClockAdjustTestParameters::new(
            skew,
            Duration::from_secs(0), // No phase correction for skew tests
            start_offset,
            start_time,
        );

        for expected_change in expected_changes_in_offset {
            let time = params.get_start_time() + expected_change.time_since_start;
            let res = params.expected_offset_change_due_to_skew_at(time);

            // Allow for small floating point precision differences
            let diff = if res > expected_change.offset_change {
                res - expected_change.offset_change
            } else {
                expected_change.offset_change - res
            };

            assert!(
                diff < Duration::from_nanos(10),
                "Expected offset change: {}ns, got: {}ns, diff: {}ns (skew: {} ppm, time_since_start: {}s)",
                expected_change.offset_change.as_nanos(),
                res.as_nanos(),
                diff.as_nanos(),
                skew.get() * 1_000_000.0, // Convert to ppm for display
                expected_change.time_since_start.as_seconds_f64()
            );
        }
    }
}
