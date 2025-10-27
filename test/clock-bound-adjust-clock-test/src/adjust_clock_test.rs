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
use clock_bound::daemon::time::{Duration, Instant, tsc::Skew};

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
    ///
    /// We need this to extrapolate the change in clock offsets
    /// at some future `Instant`.
    start_time: Instant,
    /// Initial offset of the `CLOCK_REALTIME` and `CLOCK_MONOTONIC` at
    /// test start.
    ///
    /// We need this to extrapolate the change in clock offsets
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
    fn expected_offset_change_due_to_phase_correction_at(&self, _time: Instant) -> Duration {
        todo!("implement me");
    }

    /// Returns the expected change in the offset between `CLOCK_REALTIME` and `CLOCK_MONOTONIC_RAW` based on the
    /// `skew` factor. This is much more straightforward - `freq` in the kernel is applied immediately, so we can
    /// just do a simple linear extrapolation of the expected time.
    fn expected_offset_change_due_to_skew_at(&self, _time: Instant) -> Duration {
        todo!("implement me");
    }
}
