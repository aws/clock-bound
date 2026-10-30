//! Adjust system clock.
//!
//! See [`state_machine`] for the full description of the state machine logic.

mod ntp_adjtime;
mod state_machine;

pub use ntp_adjtime::KAPIClockAdjuster;
pub use ntp_adjtime::{NtpAdjTimeError, NtpAdjTimeExt};

use crate::{daemon::clock_parameters::ClockParameters, shm::ClockStatus};
use state_machine::State;

#[cfg(test)]
pub(crate) use ntp_adjtime::MockNtpAdjTimeExt;

/// Component which adjusts the system clock
///
/// The system clock is known as the `CLOCK_REALTIME` on Unix systems
pub(crate) struct SystemClockDiscipliner {
    /// Holds the current known state of this component
    state: State,
    /// The system-specific interface to read and adjust the system clock
    ntp_adjtime: Box<dyn NtpAdjTimeExt>,
}

impl SystemClockDiscipliner {
    pub(crate) fn new(ntp_adjtime: Box<dyn NtpAdjTimeExt>) -> Self {
        Self {
            state: State::initializing(),
            ntp_adjtime,
        }
    }

    /// Convenience constructor with a `ntp_adjtime` interface for adjusting the system clock
    #[cfg(not(feature = "test-side-by-side"))]
    pub(crate) fn with_kernel_clock_adjuster() -> Self {
        Self::new(Box::new(KAPIClockAdjuster))
    }

    /// Convenience constructor with a `ntp_adjtime` interface that does nothing
    #[cfg(feature = "test-side-by-side")]
    pub(crate) fn with_noop_clock_adjuster() -> Self {
        Self::new(Box::new(ntp_adjtime::NoopClockAdjuster))
    }
}

impl SystemClockDiscipliner {
    /// Apply a correction based off of the new `ClockParameters`
    pub(crate) fn handle_clock_parameters(
        &mut self,
        clock_parameters: &ClockParameters,
    ) -> ClockCorrection {
        match &self.state {
            State::Initializing(initializing) => {
                let (running, correction) =
                    initializing.to_running(self.ntp_adjtime.as_ref(), clock_parameters);
                self.state = running.into();
                correction
            }
            State::Running(running) => {
                running.apply_offset(self.ntp_adjtime.as_ref(), clock_parameters)
            }
        }
    }

    /// Cleanup during a disruption event
    pub(crate) fn handle_disruption(&mut self, _new_disruption_marker: u64) {
        // TODO: should probably stop active corrections. Clock is unknown, so
        // we could be "correcting" in the wrong direction

        // No action taken on disruption - continue to use the PLL for corrections.
        // Clock will converge over time.
        let Self {
            state: _,
            ntp_adjtime: _,
        } = self;
    }

    /// The synchronization status of the REALTIME clock
    pub(crate) fn get_clock_realtime_status(&self) -> ClockStatus {
        match self.state {
            State::Initializing(_) => ClockStatus::Unknown,
            State::Running(_) => ClockStatus::Synchronized,
        }
    }
}

/// Method and magnitude of a `CLOCK_REALTIME` correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockCorrection {
    Step { step_ns: i64 },
    Smooth { correction_ns: i64 },
}

#[cfg(test)]
mod test {
    use crate::daemon::{
        clock_state::system_clock_discipliner::ntp_adjtime::MockNtpAdjTimeExt,
        time::{
            Clock, Duration, TscCount,
            tsc::{Frequency, Period},
        },
    };

    use super::*;

    fn clock_params_with_offset(target_offset: Duration) -> ClockParameters {
        let tsc_now = TscCount::new(crate::daemon::io::tsc::read_timestamp_counter_begin() as i64);
        let realtime_now = crate::daemon::time::clocks::RealTime.get_time();
        ClockParameters {
            tsc_count: tsc_now,
            period: Period::from_frequency(Frequency::from_hz(1_000_000_000.0)),
            time: realtime_now + target_offset,
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
            as_of_monotonic: realtime_now,
        }
    }

    #[test]
    fn initializing_transitions_to_running_on_first_event() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_smooth()
            .once()
            .returning(|_, _| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let mut adjuster = SystemClockDiscipliner::new(Box::new(mock));
        assert!(matches!(adjuster.state, State::Initializing(_)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(10)));
        assert!(matches!(adjuster.state, State::Running(_)));
    }

    #[test]
    fn running_applies_offset_on_each_tick() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_smooth()
            .times(3)
            .returning(|_, _| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let mut adjuster = SystemClockDiscipliner::new(Box::new(mock));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(10)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(10)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(10)));
        assert!(matches!(adjuster.state, State::Running(_)));
    }

    #[test]
    fn status_unknown_when_initializing_synchronized_when_running() {
        let mock = MockNtpAdjTimeExt::new();
        let adjuster = SystemClockDiscipliner::new(Box::new(mock));
        assert_eq!(adjuster.get_clock_realtime_status(), ClockStatus::Unknown);

        let mock2 = MockNtpAdjTimeExt::new();
        let adjuster2 = SystemClockDiscipliner {
            state: State::running(),
            ntp_adjtime: Box::new(mock2),
        };
        assert_eq!(
            adjuster2.get_clock_realtime_status(),
            ClockStatus::Synchronized
        );
    }
}
