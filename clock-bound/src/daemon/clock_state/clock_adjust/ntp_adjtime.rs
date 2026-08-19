//! Adjust system clock
use errno::Errno;
use libc::{TIME_DEL, TIME_ERROR, TIME_INS, TIME_OK, TIME_OOP, TIME_WAIT, ntp_adjtime};
use thiserror::Error;
use tracing::debug;

use crate::daemon::time::{Duration, timex::Timex};

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
impl NtpAdjTimeExt for KAPIClockAdjuster {}

/// Noop Clock Adjuster, which doesn't actually adjust the clock parameters but just
/// returns `TIME_OK`.
#[cfg_attr(not(feature = "test-side-by-side"), expect(unused))]
pub struct NoopClockAdjuster;
impl NtpAdjTime for NoopClockAdjuster {
    fn ntp_adjtime(&self, _tx: &mut Timex) -> i32 {
        TIME_OK
    }
}
#[cfg(feature = "test-side-by-side")]
impl NtpAdjTimeExt for NoopClockAdjuster {
    fn apply_phase_correction_step(
        &self,
        _phase_correction: Duration,
    ) -> Result<Timex, NtpAdjTimeError> {
        Ok(Timex::retrieve())
    }
}

/// Lightweight trait around `ntp_adjtime` function (formerly `adjtimex`).
/// Useful for mocking, or potentially as an abstraction around modifying
/// other clocks' parameters in the future.
#[cfg_attr(test, mockall::automock)]
pub trait NtpAdjTime {
    fn ntp_adjtime(&self, tx: &mut Timex) -> i32;
}

#[cfg(test)]
impl NtpAdjTimeExt for MockNtpAdjTime {}

#[cfg(test)]
mod mock_ntp_adj_time_ext {
    use super::*;
    mockall::mock! {
        pub NtpAdjTimeExt {}
        impl NtpAdjTimeExt for NtpAdjTimeExt {
            fn apply_phase_correction_smooth(&self, phase_correction: Duration) -> Result<Timex, NtpAdjTimeError>;
            fn apply_phase_correction_step(&self, phase_correction: Duration) -> Result<Timex, NtpAdjTimeError>;
            fn read_adjtime(&self) -> Result<Timex, NtpAdjTimeError>;
        }
    }
    impl NtpAdjTime for MockNtpAdjTimeExt {
        fn ntp_adjtime(&self, _tx: &mut Timex) -> i32 {
            unimplemented!("mocks shouldn't call this")
        }
    }
}

#[cfg(test)]
pub use mock_ntp_adj_time_ext::MockNtpAdjTimeExt;

pub trait NtpAdjTimeExt: NtpAdjTime {
    /// Feeds a measured phase offset into the kernel PLL, causing `CLOCK_REALTIME` to
    /// slew toward the reference over time (smooth correction). The kernel also
    /// applies a dampened frequency correction. Calling this repeatedly with the phase
    /// between `CLOCK_REALTIME` and `ClockBound` will eventually converge both phase
    /// and frequency.
    ///
    /// # Errors
    /// `NtpAdjTimeError::Failure` if `ntp_adjtime` returns -1
    /// `NtpAdjTimeError::BadState` if an unexpected state is returned
    /// `NtpAdjTimeError::InvalidState` if an invalid state is returned
    fn apply_phase_correction_smooth(
        &self,
        phase_correction: Duration,
    ) -> Result<Timex, NtpAdjTimeError> {
        let mut tx = Timex::apply_phase_correction(phase_correction);
        debug!("Calling ntp_adjtime to smooth-apply phase correction {phase_correction:?}.");
        match self.ntp_adjtime(&mut tx) {
            TIME_OK => Ok(tx),
            cs @ (TIME_ERROR | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                Err(NtpAdjTimeError::BadState(cs))
            }
            -1 => Err(NtpAdjTimeError::Failure(errno::errno())),
            unexpected => Err(NtpAdjTimeError::InvalidState(unexpected)),
        }
    }

    /// Applies an instantaneous step of `CLOCK_REALTIME` by the given offset.
    ///
    /// # Errors
    /// `NtpAdjTimeError::Failure` if `ntp_adjtime` returns -1
    /// `NtpAdjTimeError::BadState` if an unexpected state is returned
    /// `NtpAdjTimeError::InvalidState` if an invalid state is returned
    fn apply_phase_correction_step(
        &self,
        phase_correction: Duration,
    ) -> Result<Timex, NtpAdjTimeError> {
        let mut tx = Timex::clock_step()
            .phase_correction(phase_correction)
            .call();
        debug!("Calling ntp_adjtime to step-apply phase correction {phase_correction:?}.");
        // TIME_ERROR is expected after a step — the clock is now "unsynchronized"
        match self.ntp_adjtime(&mut tx) {
            TIME_ERROR => Ok(tx),
            cs @ (TIME_OK | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT) => {
                Err(NtpAdjTimeError::BadState(cs))
            }
            -1 => Err(NtpAdjTimeError::Failure(errno::errno())),
            unexpected => Err(NtpAdjTimeError::InvalidState(unexpected)),
        }
    }

    /// Reads the current set of kernel clock adjustment variables without modifying them.
    /// Useful for gathering metrics on the current kernel timekeeping state.
    ///
    /// # Errors
    /// `NtpAdjTimeError::Failure` if `ntp_adjtime` returns -1
    /// `NtpAdjTimeError::InvalidState` if an invalid state is returned
    fn read_adjtime(&self) -> Result<Timex, NtpAdjTimeError> {
        let mut tx = Timex::retrieve();
        debug!("Calling ntp_adjtime to retrieve kernel params.");
        match self.ntp_adjtime(&mut tx) {
            TIME_OK | TIME_ERROR | TIME_INS | TIME_DEL | TIME_OOP | TIME_WAIT => Ok(tx),
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
    #[case::positive(Duration::from_nanos(500))]
    #[case::negative(Duration::from_nanos(-500))]
    #[case::zero(Duration::from_nanos(0))]
    fn apply_phase_correction_smooth_happy_path(#[case] phase_correction: Duration) {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(TIME_OK);
        mock.apply_phase_correction_smooth(phase_correction)
            .unwrap();
    }

    #[test]
    fn apply_phase_correction_smooth_failure() {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(-1);
        assert!(matches!(
            mock.apply_phase_correction_smooth(Duration::from_nanos(500))
                .unwrap_err(),
            NtpAdjTimeError::Failure(_)
        ));
    }

    #[rstest]
    #[case::positive(Duration::from_millis(100))]
    #[case::negative(-Duration::from_millis(100))]
    #[case::zero(Duration::from_millis(0))]
    fn apply_phase_correction_step_happy_path(#[case] phase_correction: Duration) {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(TIME_ERROR);
        mock.apply_phase_correction_step(phase_correction).unwrap();
    }

    #[test]
    fn apply_phase_correction_step_failure() {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(-1);
        assert!(matches!(
            mock.apply_phase_correction_step(Duration::from_millis(100))
                .unwrap_err(),
            NtpAdjTimeError::Failure(_)
        ));
    }

    #[test]
    fn read_adjtime_happy_path() {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(TIME_OK);
        mock.read_adjtime().unwrap();
    }

    #[test]
    fn read_adjtime_failure() {
        let mut mock = MockNtpAdjTime::new();
        mock.expect_ntp_adjtime().times(1).return_const(-1);
        assert!(matches!(
            mock.read_adjtime().unwrap_err(),
            NtpAdjTimeError::Failure(_)
        ));
    }
}
