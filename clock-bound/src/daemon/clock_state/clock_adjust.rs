//! Adjust system clock
use libc::{TIME_DEL, TIME_ERROR, TIME_INS, TIME_OK, TIME_OOP, TIME_WAIT};
use tracing::{debug, error, info};

use crate::daemon::{
    clock_parameters::ClockParameters,
    time::{Duration, inner::ClockOffsetAndRtt, instant::Utc, timex::Timex, tsc::Skew},
};

mod ntp_adjtime;
pub use ntp_adjtime::{
    KAPIClockAdjuster, NoopClockAdjuster, NtpAdjTime, NtpAdjTimeError, NtpAdjTimeExt,
};

pub struct ClockAdjuster<T> {
    ntp_adjtime: T,
    should_step: bool,
}

#[cfg_attr(test, mockall::automock)]
#[expect(clippy::missing_errors_doc, reason = "tphan to update")]
pub trait ClockAdjust {
    fn handle_clock_parameters(
        &mut self,
        clock_parameters: &ClockParameters,
        clock_realtime_offset_and_rtt: ClockOffsetAndRtt<Utc>,
    ) -> Result<Timex, NtpAdjTimeError>;
    fn handle_disruption(&mut self, new_disruption_marker: u64);
}

impl<T: NtpAdjTime> ClockAdjust for ClockAdjuster<T> {
    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Errors
    /// This method returns [`NtpAdjTimeError`] if the call has failed or has an unexpected return code.
    fn handle_clock_parameters(
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

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    ///
    /// # Panics
    /// If `adjust_clock` fails, e.g. an invalid value was supplied to `ntp_adjtime` (unlikely), or
    /// insufficient permissions to adjust the clock.
    fn handle_disruption(&mut self, _new_disruption_marker: u64) {
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

impl<T: NtpAdjTime> ClockAdjuster<T> {
    pub fn new(ntp_adjtime: T) -> Self {
        // Should step on first adjustment.
        let should_step = true;
        Self {
            ntp_adjtime,
            should_step,
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

        debug!(
            "calling ntp_adjtime to adjust clock with phase_correction {phase_correction:?} and skew {skew:?}"
        );
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

        debug!(
            "calling ntp_adjtime to step clock with phase_correction {phase_correction:?} {:?}",
            tx
        );
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
}

#[cfg(test)]
mod test {
    use mockall::predicate::eq;

    use crate::daemon::clock_state::clock_adjust::ntp_adjtime::MockNtpAdjTime;

    use super::*;

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
