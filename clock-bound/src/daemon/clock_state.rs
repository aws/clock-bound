//! Adjust system clock and clockbound shared memory
use errno::Errno;
use libc::{
    ADJ_FREQUENCY, ADJ_NANO, ADJ_OFFSET, ADJ_STATUS, ADJ_TIMECONST, STA_FREQHOLD, STA_PLL,
    TIME_DEL, TIME_ERROR, TIME_INS, TIME_OK, TIME_OOP, TIME_WAIT, ntp_adjtime, timex,
};
use thiserror::Error;
use tracing::debug;

use crate::daemon::time::{Duration, tsc::Skew};

/// Lightweight trait around `ntp_adjtime` function (formerly `adjtimex`).
/// Useful for mocking, or potentially as an abstraction around modifying
/// other clocks' parameters in the future.
#[cfg_attr(test, mockall::automock)]
pub trait ClockAdjuster {
    unsafe fn ntp_adjtime(&self, timex: *mut timex) -> i32;
}

/// Concrete struct implementing `ntp_adjtime` by delegating to the `libc`
/// implementation. Should be the only actual concrete implementation.
pub struct ClockAdjusterImpl;
impl ClockAdjuster for ClockAdjusterImpl {
    unsafe fn ntp_adjtime(&self, timex: *mut timex) -> i32 {
        // Safety: `timex` should be initialized by the caller
        unsafe { ntp_adjtime(timex) }
    }
}

/// Error type returned when dealing with underlying `adjtimex` or `ntp_adjtime`
/// results.
#[derive(Debug, Error)]
pub enum AdjTimexError {
    #[error("Failed to adjust the clock")]
    Failure(Errno),
    #[error("Unexpected bad state return value from ntp_adjtime")]
    BadState(i32),
    #[error("Invalid return value from ntp_adjtime")]
    InvalidState(i32),
}

/// Struct which handles adjusting the system clock using `ntp_adjtime`, and
/// modifications of the SHM segment together (for now). The two should likely be decoupled
/// once the ClockBound Client implementation no longer depends on the kernel
/// clocks.
pub struct ClockState;

impl ClockState {
    /// Performs an adjustment of the clock, to apply the given phase correction
    /// and skew values, in a single system call.
    ///
    /// The phase correction will be passed directly to the kernel PLL to correct the system
    /// clock via a slew with exponential decaying effect (the proportion % corrected per second
    /// is controlled by `tx.constant`).
    ///
    /// The skew, or frequency error relative to a baseline at 0, is applied directly
    /// to the `freq` in the timekeeping utilities, e.g. passing +1ppm will "speed up" the clock
    /// by 1ppm. Thus, if the clock is slow by 1 microsecond every second, we should pass in +1ppm.
    pub fn adjust_clock(
        &self,
        clock_adjuster: impl ClockAdjuster,
        phase_correction: Duration,
        skew: Skew,
    ) -> Result<(), AdjTimexError> {
        let mut tx: timex = unsafe { std::mem::MaybeUninit::zeroed().assume_init() };
        tx.freq = skew.to_timex_freq();
        tx.offset = phase_correction.as_nanos() as i64;

        // PLL clock adjustment proportion is dependent on this time constant.
        // The clock adjustment factor over the length of a second
        // is calculated as `shift_right(offset, SHIFT_PLL + ntpdata->time_constant)`, where const `SHIFT_PLL` = 2
        // So, if we want to correct the clock quickly, we use a lower time constant.
        // The value is clamped between 0 and 10.
        // For now, we use 0, to aggressively correct the clock, which means we'd expect for
        // offset to be corrected by `offset >> 2` every second.
        tx.constant = 0;

        // Set `modes` bits for all fields we modify, plus ADJ_NANO to use nanosecond units
        // and ADJ_STATUS to set status bits below.
        tx.modes |= ADJ_FREQUENCY;
        tx.modes |= ADJ_OFFSET;
        tx.modes |= ADJ_TIMECONST;
        tx.modes |= ADJ_NANO;
        tx.modes |= ADJ_STATUS;

        // Hold the frequency that we prescribe, if this is omitted the PLL would modify `freq`
        // which we do not want since ClockBound's clock sync algorithm should determine the proper
        // frequency setting.
        tx.status |= STA_FREQHOLD;
        // Only rely on PLL to perform phase adjustments
        tx.status |= STA_PLL;

        // Make the system call
        unsafe {
            debug!("calling ntp_adjtime with {:?}", tx);
            match clock_adjuster.ntp_adjtime(&mut tx) {
                TIME_OK => Ok(()),
                cs @ (TIME_ERROR | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                    Err(AdjTimexError::BadState(cs))
                }
                -1 => Err(AdjTimexError::Failure(errno::errno())),
                unexpected => Err(AdjTimexError::InvalidState(unexpected)),
            }
        }
    }
}

#[cfg(test)]
mod test {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case::positives(Duration::from_nanos(500), Skew::from_ppm(1.0))]
    #[case::negatives(Duration::from_nanos(-500), Skew::from_ppm(-1.0))]
    #[case::zeroes(Duration::from_nanos(0), Skew::from_ppm(0.0))]
    #[case::positive_offset_negative_skew(Duration::from_nanos(500), Skew::from_ppm(-1.0))]
    #[case::negative_offset_positive_skew(Duration::from_nanos(-500), Skew::from_ppm(1.0))]
    fn adjust_clock_happy_paths(
        #[case] input_phase_correction: Duration,
        #[case] input_skew: Skew,
    ) {
        let mut mock_clock_adjuster = MockClockAdjuster::new();
        let clock_state = ClockState;

        // Set up mock expectations
        mock_clock_adjuster
            .expect_ntp_adjtime()
            .withf(move |tx: &*mut timex| {
                let tx = unsafe { **tx };
                debug!("{tx:?}");
                // Verify the timex struct is configured correctly
                tx.freq == input_skew.to_timex_freq()
                    && tx.offset as i128 == input_phase_correction.as_nanos()
                    && tx.constant == 0
                    && tx.modes & ADJ_FREQUENCY != 0
                    && tx.modes & ADJ_OFFSET != 0
                    && tx.modes & ADJ_TIMECONST != 0
                    && tx.modes & ADJ_NANO != 0
                    && tx.modes & ADJ_STATUS != 0
                    && tx.status & STA_FREQHOLD != 0
                    && tx.status & STA_PLL != 0
            })
            .times(1)
            .return_const(TIME_OK);

        // Call adjust_clock with test values
        let result =
            clock_state.adjust_clock(mock_clock_adjuster, input_phase_correction, input_skew);

        assert!(result.is_ok());
    }

    #[test]
    fn adjust_clock_failure() {
        let mut mock_clock_adjuster = MockClockAdjuster::new();
        let clock_state = ClockState;

        // Set up mock expectations
        mock_clock_adjuster
            .expect_ntp_adjtime()
            .times(1)
            .return_const(-1);

        // Call adjust_clock with test values
        let result = clock_state.adjust_clock(
            mock_clock_adjuster,
            Duration::from_nanos(500),
            Skew::from_ppm(1.0),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AdjTimexError::Failure(_)));
    }

    #[test]
    fn adjust_clock_bad_state() {
        let mut mock_clock_adjuster = MockClockAdjuster::new();
        let clock_state = ClockState;

        // Set up mock expectations
        mock_clock_adjuster
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_ERROR);

        // Call adjust_clock with test values
        let result = clock_state.adjust_clock(
            mock_clock_adjuster,
            Duration::from_nanos(500),
            Skew::from_ppm(1.0),
        );

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AdjTimexError::BadState(_)));
    }

    #[test]
    fn adjust_clock_unexpected_value() {
        let mut mock_clock_adjuster = MockClockAdjuster::new();
        let clock_state = ClockState;

        // Set up mock expectations
        mock_clock_adjuster
            .expect_ntp_adjtime()
            .times(1)
            .return_const(12345);

        // Call adjust_clock with test values
        let result = clock_state.adjust_clock(
            mock_clock_adjuster,
            Duration::from_nanos(500),
            Skew::from_ppm(1.0),
        );

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AdjTimexError::InvalidState(_)
        ));
    }
}
