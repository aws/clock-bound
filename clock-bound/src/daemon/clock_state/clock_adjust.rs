//! Adjust system clock.
//!
//! See [`state_machine`] for the full description of the state machine logic.

use crate::{daemon::clock_parameters::ClockParameters, shm::ClockStatus};

mod ntp_adjtime;
mod state_machine;
pub use ntp_adjtime::KAPIClockAdjuster;
#[cfg(feature = "test-side-by-side")]
pub use ntp_adjtime::NoopClockAdjuster;
pub use ntp_adjtime::{NtpAdjTimeError, NtpAdjTimeExt};

use state_machine::{Initializing, State};

/// Method and magnitude of a `CLOCK_REALTIME` correction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClockCorrection {
    Step { step_ns: i64 },
    Smooth { correction_ns: i64 },
}

#[cfg_attr(test, mockall::automock)]
pub(crate) trait ClockAdjust: Send + Sync {
    fn handle_clock_parameters(&mut self, clock_parameters: &ClockParameters) -> ClockCorrection;
    fn handle_disruption(&mut self, new_disruption_marker: u64);
    /// Returns `Unknown` until the first successful adjustment, `Synchronized` thereafter.
    fn get_clock_realtime_status(&self) -> ClockStatus;
}

pub struct ClockAdjuster<T> {
    state: State,
    ntp_adjtime: T,
}

impl<T: NtpAdjTimeExt> ClockAdjuster<T> {
    pub fn new(ntp_adjtime: T) -> Self {
        Self {
            state: State::Initializing(Initializing),
            ntp_adjtime,
        }
    }
}

impl<T: NtpAdjTimeExt + Send + Sync> ClockAdjust for ClockAdjuster<T> {
    fn handle_clock_parameters(&mut self, clock_parameters: &ClockParameters) -> ClockCorrection {
        let (new_state, correction) = match &self.state {
            State::Initializing(inner) => {
                let (running, correction) = inner.to_running(&self.ntp_adjtime, clock_parameters);
                (State::Running(running), correction)
            }
            State::Running(inner) => {
                let (running, correction) = inner.apply_offset(&self.ntp_adjtime, clock_parameters);
                (State::Running(running), correction)
            }
        };
        self.state = new_state;
        correction
    }

    fn handle_disruption(&mut self, _new_disruption_marker: u64) {
        // No action taken on disruption - continue to use the PLL for corrections.
        // Clock will converge over time.
        let Self {
            state: _,
            ntp_adjtime: _,
        } = self;
    }

    fn get_clock_realtime_status(&self) -> ClockStatus {
        match self.state {
            State::Initializing(_) => ClockStatus::Unknown,
            State::Running(_) => ClockStatus::Synchronized,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::daemon::{
        clock_state::clock_adjust::ntp_adjtime::MockNtpAdjTimeExt,
        clock_state::clock_adjust::state_machine::Running,
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

        let mut adjuster = ClockAdjuster::new(mock);
        assert!(matches!(adjuster.state, State::Initializing(_)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(100)));
        assert!(matches!(adjuster.state, State::Running(_)));
    }

    #[test]
    fn running_applies_offset_on_each_tick() {
        let mut mock = MockNtpAdjTimeExt::new();
        mock.expect_apply_phase_correction_smooth()
            .times(3)
            .returning(|_, _| Ok(crate::daemon::time::timex::Timex::retrieve()));

        let mut adjuster = ClockAdjuster::new(mock);
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(100)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(100)));
        adjuster.handle_clock_parameters(&clock_params_with_offset(Duration::from_millis(100)));
        assert!(matches!(adjuster.state, State::Running(_)));
    }

    #[test]
    fn status_unknown_when_initializing_synchronized_when_running() {
        let mock = MockNtpAdjTimeExt::new();
        let adjuster = ClockAdjuster::new(mock);
        assert_eq!(adjuster.get_clock_realtime_status(), ClockStatus::Unknown);

        let mock2 = MockNtpAdjTimeExt::new();
        let adjuster2 = ClockAdjuster {
            state: State::Running(Running),
            ntp_adjtime: mock2,
        };
        assert_eq!(
            adjuster2.get_clock_realtime_status(),
            ClockStatus::Synchronized
        );
    }
}
