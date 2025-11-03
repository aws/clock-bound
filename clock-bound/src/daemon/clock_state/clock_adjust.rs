//! Adjust system clock
use errno::Errno;
use libc::{TIME_DEL, TIME_ERROR, TIME_INS, TIME_OK, TIME_OOP, TIME_WAIT, ntp_adjtime};
use thiserror::Error;
use tracing::{debug, error, info};

use crate::daemon::{
    clock_parameters::ClockParameters,
    time::{Duration, inner::ClockOffsetAndRtt, instant::Utc, timex::Timex, tsc::Skew},
};

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
    should_step: bool,
}

#[cfg(test)]
mockall::mock! {
    pub ClockAdjuster<T> {
        pub fn handle_clock_parameters(
            &mut self,
            // This is needed to tell ClockAdjust what frequency to use.
            _clock_parameters: &ClockParameters,
            // This is needed to tell ClockAdjust what phase offset to use.
            clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
        ) -> Result<Timex, NtpAdjTimeError>;
        pub fn handle_disruption(&mut self, new_disruption_marker: u64);
    }
}

impl<T: NtpAdjTime> ClockAdjuster<T> {
    pub fn new(ntp_adjtime: T) -> Self {
        // Should step on first adjustment.
        let should_step = true;
        Self {
            ntp_adjtime,
            should_step,
        }
    }

    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Errors
    /// This method returns [`NtpAdjTimeError`] if the call has failed or has an unexpected return code.
    pub fn handle_clock_parameters(
        &mut self,
        // This is needed to tell ClockAdjust what frequency to use.
        _clock_parameters: &ClockParameters,
        // This is needed to tell ClockAdjust what phase offset to use.
        clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
    ) -> Result<Timex, NtpAdjTimeError> {
        if self.should_step {
            self.step_clock(clock_realtime_offset_and_rtt.offset())
        } else {
            self.adjust_clock(clock_realtime_offset_and_rtt.offset(), Skew::from_ppm(0.0))
        }
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
    ) -> Result<Timex, NtpAdjTimeError> {
        let mut tx = Timex::clock_adjustment()
            .phase_correction(phase_correction)
            .skew(skew)
            .call();

        debug!("calling ntp_adjtime with {:?}", tx);
        match self.ntp_adjtime.ntp_adjtime(&mut tx) {
            TIME_OK => Ok(tx),
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
    pub fn step_clock(&mut self, phase_correction: Duration) -> Result<Timex, NtpAdjTimeError> {
        let mut tx = Timex::clock_step()
            .phase_correction(phase_correction)
            .call();

        debug!("calling ntp_adjtime with {:?}", tx);
        // NOTE: we actually expect TIME_ERROR if the clock adjustment succeeds, since
        // that indicates the clock is now "unsynchronized" (expected after we step the clock
        // discontinuously)
        match self.ntp_adjtime.ntp_adjtime(&mut tx) {
            TIME_ERROR => {
                // Step was successful, so we should not step again.
                self.should_step = false;
                Ok(tx)
            }
            cs @ (TIME_OK | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                Err(NtpAdjTimeError::BadState(cs))
            }
            -1 => Err(NtpAdjTimeError::Failure(errno::errno())),
            unexpected => Err(NtpAdjTimeError::InvalidState(unexpected)),
        }
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    ///
    /// # Panics
    /// If `adjust_clock` fails, e.g. an invalid value was supplied to `ntp_adjtime` (unlikely), or
    /// insufficient permissions to adjust the clock.
    pub fn handle_disruption(&mut self, _new_disruption_marker: u64) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field to Self without handling it here
        let Self {
            ntp_adjtime: _,
            should_step: _,
        } = self;
        // At least stop any ongoing phase correction slew or frequency correction, if the clock is disrupted.
        // Notably, phase correction slew is recalculated at the top of a second, so we still might end up having some moderate slew
        // of the clock happening til that time.
        info!("Resetting ntp_adjtime parameters to zero any phase or frequency corrections");
        match self.adjust_clock(Duration::from_secs(0), Skew::from_ppm(0.0)) {
            failed_adjtime @ Err(NtpAdjTimeError::Failure(_)) => {
                failed_adjtime.unwrap();
            }
            Err(unexpected_adjtime_status) => {
                error!("Unexpected adjtime result: {unexpected_adjtime_status}");
            }
            _ => {}
        }
        // TODO: We may want to reset `should_step` if we think it is acceptable to step the clock on next adjustment
        // for faster recovery..
        tracing::info!("Handled clock disruption event");
    }
}

#[cfg(test)]
mod test {
    use mockall::predicate::eq;
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
        assert!(clock_adjuster.should_step);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_OK);

        // Call adjust_clock with test values
        let result = clock_adjuster.adjust_clock(input_phase_correction, input_skew);

        assert!(result.is_ok());
        assert!(clock_adjuster.should_step);
    }

    #[test]
    fn adjust_clock_failure() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        assert!(clock_adjuster.should_step);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(-1);

        // Call adjust_clock with test values
        assert!(matches!(
            clock_adjuster
                .adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0))
                .unwrap_err(),
            NtpAdjTimeError::Failure(_)
        ));
        assert!(clock_adjuster.should_step);
    }

    #[test]
    fn adjust_clock_bad_state() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        assert!(clock_adjuster.should_step);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_ERROR);

        // Call adjust_clock with test values
        assert!(matches!(
            clock_adjuster
                .adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0))
                .unwrap_err(),
            NtpAdjTimeError::BadState(_)
        ));
        assert!(clock_adjuster.should_step);
    }

    #[test]
    fn adjust_clock_unexpected_value() {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        assert!(clock_adjuster.should_step);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(12345);

        // Call adjust_clock with test values
        assert!(matches!(
            clock_adjuster
                .adjust_clock(Duration::from_nanos(500), Skew::from_ppm(1.0))
                .unwrap_err(),
            NtpAdjTimeError::InvalidState(_)
        ));
        assert!(clock_adjuster.should_step);
    }

    #[rstest]
    #[case::positive(Duration::from_millis(100))]
    #[case::negative(-Duration::from_millis(100))]
    #[case::zero(Duration::from_millis(0))]
    fn step_clock_happy_paths(#[case] input_phase_correction: Duration) {
        let mock_ntp_adj_time = MockNtpAdjTime::new();
        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        assert!(clock_adjuster.should_step);

        // Set up mock expectations
        clock_adjuster
            .ntp_adjtime
            .expect_ntp_adjtime()
            .times(1)
            .return_const(TIME_ERROR);

        // Call step_clock with test values
        clock_adjuster.step_clock(input_phase_correction).unwrap();
        assert!(!clock_adjuster.should_step);
    }

    #[test]
    fn handle_disruption() {
        let disruption_marker = 123;
        let expected_tx = Timex::clock_adjustment()
            .phase_correction(Duration::from_secs(0))
            .skew(Skew::from_ppm(0.0))
            .call();
        let mut mock_ntp_adj_time = MockNtpAdjTime::new();
        mock_ntp_adj_time
            .expect_ntp_adjtime()
            .once()
            .with(eq(expected_tx))
            .return_once(|_| 0);

        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        clock_adjuster.handle_disruption(disruption_marker);
    }

    #[test]
    #[should_panic]
    fn handle_disruption_panics_if_hard_failure_to_adjust_clock() {
        let disruption_marker = 123;
        let expected_tx = Timex::clock_adjustment()
            .phase_correction(Duration::from_secs(0))
            .skew(Skew::from_ppm(0.0))
            .call();
        let mut mock_ntp_adj_time = MockNtpAdjTime::new();
        mock_ntp_adj_time
            .expect_ntp_adjtime()
            .once()
            .with(eq(expected_tx))
            .return_once(|_| -1);

        let mut clock_adjuster = ClockAdjuster::new(mock_ntp_adj_time);
        clock_adjuster.handle_disruption(disruption_marker);
    }
}
