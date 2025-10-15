//! Adjust system clock and clockbound shared memory
use errno::Errno;
use libc::{TIME_DEL, TIME_ERROR, TIME_INS, TIME_OK, TIME_OOP, TIME_WAIT, ntp_adjtime};
use thiserror::Error;
use tracing::debug;

use crate::daemon::time::{Duration, timex::Timex, tsc::Skew};

/// Error type returned when dealing with underlying `adjtimex` or `ntp_adjtime`
/// results.
#[derive(Debug, Error)]
pub enum NtpAdjTimeError {
    #[error("Failed to adjust the clock: {0}")]
    Failure(Errno),
    #[error("Unexpected bad state return value from ntp_adjtime: {0}")]
    BadState(i32),
    #[error("Invalid return value from ntp_adjtime: {0}")]
    InvalidState(i32),
}

/// Concrete struct implementing `ntp_adjtime` by delegating to the `libc`
/// implementation. Should be the only actual concrete implementation.
pub struct KAPIClockAdjuster;
impl NtpAdjTime for KAPIClockAdjuster {
    fn ntp_adjtime(&self, tx: &mut Timex) -> i32 {
        // # Safety
        // `tx` should point to a valid struct because of validation guarantees of `Timex`
        unsafe { ntp_adjtime(tx.expose()) }
    }
}

/// Noop Clock Adjuster, which doesn't actually adjust the clock parameters but just
/// returns `TIME_OK`.
pub struct NoopClockAdjuster;
impl NtpAdjTime for NoopClockAdjuster {
    fn ntp_adjtime(&self, _tx: &mut Timex) -> i32 {
        TIME_OK
    }
}

/// Lightweight trait around `ntp_adjtime` function (formerly `adjtimex`).
/// Useful for mocking, or potentially as an abstraction around modifying
/// other clocks' parameters in the future.
#[cfg_attr(test, mockall::automock)]
pub trait NtpAdjTime {
    fn ntp_adjtime(&self, tx: &mut Timex) -> i32;
}

pub struct ClockAdjuster<T> {
    ntp_adjtime: T,
}

impl<T: NtpAdjTime> ClockAdjuster<T> {
    pub fn new(ntp_adjtime: T) -> Self {
        Self { ntp_adjtime }
    }

    /// Performs an adjustment of the clock, to apply the given phase correction
    /// and skew values, in a single system call.
    ///
    /// # Errors
    /// `NtpAdjTimeError::Failure` if `ntp_adjtime` returns -1, meaning the system call failed, along with errno
    /// `NtpAdjTimeError::BadState` if some state other than `TIME_OK` is returned from `ntp_adjtime`
    /// `NtpAdjTimeError::InvalidState` if some invalid or not well-documented state is returned from `ntp_adjtime`
    pub fn adjust_clock(
        &self,
        phase_correction: Duration,
        skew: Skew,
    ) -> Result<(), NtpAdjTimeError> {
        let mut tx = Timex::clock_adjustment()
            .phase_correction(phase_correction)
            .skew(skew)
            .call();

        debug!("calling ntp_adjtime with {:?}", tx);
        match self.ntp_adjtime.ntp_adjtime(&mut tx) {
            TIME_OK => Ok(()),
            cs @ (TIME_ERROR | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                Err(NtpAdjTimeError::BadState(cs))
            }
            -1 => Err(NtpAdjTimeError::Failure(errno::errno())),
            unexpected => Err(NtpAdjTimeError::InvalidState(unexpected)),
        }
    }

    /// Applies an instantaneous step of `CLOCK_REALTIME` based on the passed `phase_correction` value.
    ///
    /// # Errors
    /// `NtpAdjTimeError::Failure` if `ntp_adjtime` returns -1, meaning the system call failed, along with errno
    /// `NtpAdjTimeError::BadState` if some state other than `TIME_ERROR` is returned from `ntp_adjtime`
    /// `NtpAdjTimeError::InvalidState` if some invalid or not well-documented state is returned from `ntp_adjtime`
    pub fn step_clock(&self, phase_correction: Duration) -> Result<(), NtpAdjTimeError> {
        let mut tx = Timex::clock_step()
            .phase_correction(phase_correction)
            .call();

        debug!("calling ntp_adjtime with {:?}", tx);
        // NOTE: we actually expect TIME_ERROR if the clock adjustment succeeds, since
        // that indicates the clock is now "unsynchronized" (expected after we step the clock
        // discontinuously)
        match self.ntp_adjtime.ntp_adjtime(&mut tx) {
            TIME_ERROR => Ok(()),
            cs @ (TIME_OK | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                Err(NtpAdjTimeError::BadState(cs))
            }
            -1 => Err(NtpAdjTimeError::Failure(errno::errno())),
            unexpected => Err(NtpAdjTimeError::InvalidState(unexpected)),
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
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_OK);

        // Call adjust_clock with test values
        let result = clock_adjuster.adjust_clock(input_phase_correction, input_skew);

        assert!(result.is_ok());
    }

    #[test]
    fn adjust_clock_failure() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(-1);

        // Call adjust_clock with test values
        let result = clock_adjuster.adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0));

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), NtpAdjTimeError::Failure(_)));
    }

    #[test]
    fn adjust_clock_bad_state() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_ERROR);

        // Call adjust_clock with test values
        let result = clock_adjuster.adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0));

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), NtpAdjTimeError::BadState(_)));
    }

    #[test]
    fn adjust_clock_unexpected_value() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(12345);

        // Call adjust_clock with test values
        let result = clock_adjuster.adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0));

        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            NtpAdjTimeError::InvalidState(_)
        ));
    }

    #[rstest]
    #[case::positive(Duration::from_millis(100))]
    #[case::negative(-Duration::from_millis(100))]
    #[case::zero(Duration::from_millis(0))]
    fn step_clock_happy_paths(#[case] input_phase_correction: Duration) {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_ERROR);

        // Call step_clock with test values
        let result = clock_adjuster.step_clock(input_phase_correction);

        assert!(result.is_ok());
    }
}
