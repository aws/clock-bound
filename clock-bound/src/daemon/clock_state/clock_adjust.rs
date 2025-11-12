//! Adjust system clock.
//!
//! `ClockBound` tries to make `CLOCK_REALTIME` follow UTC as closely as possible.
//! It has its own notion of a "best internal clock" output by the `ClockSyncAlgorithm` and selector,
//! which is expressed as a set of `ClockParameters`, and reading that clock is done via
//! using those `ClockParameters` + corresponding TSC reads, similar to `clock_gettime` VDSO implementation.
//! To make `CLOCK_REALTIME` follow UTC, we make it follow `ClockBound` internal clock by steering it via
//! frequency and phase corrections.
//!
//! Frequency corrections are applied via `ntp_adjtime` - the frequency to apply is calculated by comparing
//! two `ClockSnapshot`s with timestamps of `CLOCK_REALTIME` along with `ClockBound` internal clock, calculating the relative
//! frequency, and applying the relative frequency change to the currently used `CLOCK_REALTIME` frequency (gathered
//! via `ntp_adjtime` as part of the `ClockSnapshot`.
//!
//! Phase corrections are applied via `ntp_adjtime` as well, and are a simpler task - we simply use the old trick
//! of interleaved reads to compare the offset of two clocks (offset of `ClockBound` w.r.t `CLOCK_REALTIME`) and apply
//! that as the phase correction to `CLOCK_REALTIME`, using the PLL slewing method in kernel. An alternative approach
//! to fixing phase corrections could involve temporary slewing of the clock frequency, but can risk overshooting, or
//! leaving the system in a bad state if in the middle of a slew and the daemon is terminated.
use tracing::{error, info, trace};

use crate::{
    daemon::{
        clock_parameters::ClockParameters,
        time::{Duration, tsc::Skew},
    },
    shm::ClockStatus,
};

mod ntp_adjtime;
mod state_machine;
pub use ntp_adjtime::{KAPIClockAdjuster, NtpAdjTimeError, NtpAdjTimeExt};

#[cfg_attr(test, mockall::automock)]
pub(crate) trait ClockAdjust: Send + Sync {
    fn handle_clock_parameters(
        &mut self,
        now: tokio::time::Instant,
        clock_parameters: &ClockParameters,
    );
    fn handle_disruption(&mut self, new_disruption_marker: u64);
    /// Helper to find out if `CLOCK_REALTIME` is now reliable...
    /// Any initial states can be considered unreliable, since we have not
    /// corrected or been able to measure the clock at all.
    fn get_clock_realtime_status(&self) -> ClockStatus;
}

pub struct ClockAdjuster<T> {
    state: State,
    ntp_adjtime: T,
}

impl<T: NtpAdjTimeExt + Send + Sync> ClockAdjust for ClockAdjuster<T> {
    fn handle_clock_parameters(
        &mut self,
        now: tokio::time::Instant,
        clock_parameters: &ClockParameters,
    ) {
        self.handle_event(now, clock_parameters);
    }

    fn handle_disruption(&mut self, _new_disruption_marker: u64) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field to Self without handling it here
        let Self {
            ntp_adjtime: _,
            state,
        } = self;
        // At least stop any ongoing phase correction slew or frequency correction, if the clock is disrupted.
        // Notably, phase correction slew is recalculated at the top of a second, so we still might end up having some moderate slew
        // of the clock happening til that time.
        info!("Resetting ntp_adjtime parameters to zero any phase or frequency corrections");
        match self
            .ntp_adjtime
            .adjust_clock(Duration::from_secs(0), Skew::from_ppm(0.0))
        {
            failed_adjtime @ Err(NtpAdjTimeError::Failure(_)) => {
                failed_adjtime.unwrap();
            }
            Err(unexpected_adjtime_status) => {
                error!("Unexpected adjtime result: {unexpected_adjtime_status}");
            }
            _ => {}
        }
        *state = State::Disrupted(Disrupted);
        // TODO: We may want to reset `should_step` if we think it is acceptable to step the clock on next adjustment
        // for faster recovery..
        info!("Handled clock disruption event");
    }

    /// Helper to find out if `CLOCK_REALTIME` is now reliable...
    /// Any initial states can be considered unreliable, since we have not
    /// corrected or been able to measure the clock at all.
    fn get_clock_realtime_status(&self) -> ClockStatus {
        match &self.state {
            State::Disrupted(_) => ClockStatus::Disrupted,
            State::Initialized(_)
            | State::InitialPhaseCorrectHalted { .. }
            | State::InitialSnapshotRetrieved { .. } => ClockStatus::Unknown,
            State::ClockAdjusted { .. }
            | State::PhaseCorrectHalted { .. }
            | State::SnapshotRetrieved { .. } => ClockStatus::Synchronized,
        }
    }
}

use state_machine::{
    ClockAdjusted, Disrupted, InitialPhaseCorrectHalted, InitialSnapshotRetrieved, Initialized,
    PhaseCorrectHalted, SnapshotRetrieved, State,
};

impl<T: NtpAdjTimeExt> ClockAdjuster<T> {
    pub fn new(ntp_adjtime: T) -> Self {
        Self {
            state: State::Initialized(Initialized),
            ntp_adjtime,
        }
    }

    /// Central event handler for `ClockAdjuster`. The state machine mostly consists of transitions
    /// based on time elapsed since last state. Handling of these deadlines, and determining whether to
    /// transition, is done centrally here.
    ///
    /// If a state matches and its criteria to transition (i.e. duration of state has passed), then
    /// we mutate our internal state.
    pub fn handle_event(&mut self, now: tokio::time::Instant, clock_params: &ClockParameters) {
        match &self.state {
            State::Disrupted(inner) => {
                self.state = State::InitialPhaseCorrectHalted(
                    inner.to_initial_phase_correct_halted(&self.ntp_adjtime),
                );
            }
            State::Initialized(inner) => {
                self.state = State::InitialPhaseCorrectHalted(
                    inner.to_initial_phase_correct_halted(&self.ntp_adjtime, clock_params),
                );
            }
            State::InitialPhaseCorrectHalted(inner) => {
                let InitialPhaseCorrectHalted { instant } = inner;
                if now.duration_since(*instant) >= InitialPhaseCorrectHalted::DURATION {
                    self.state = State::InitialSnapshotRetrieved(
                        inner.to_initial_snapshot_retrieved(&self.ntp_adjtime),
                    );
                } else {
                    trace!("No state transition expected from InitialPhaseCorrectHalted");
                }
            }
            State::InitialSnapshotRetrieved(inner) => {
                let InitialSnapshotRetrieved { instant, snapshot } = inner;
                if now.duration_since(*instant) >= InitialSnapshotRetrieved::DURATION {
                    self.state = State::ClockAdjusted(inner.to_clock_adjusted(
                        &self.ntp_adjtime,
                        clock_params,
                        snapshot,
                    ));
                } else {
                    trace!("No state transition expected from InitialSnapshotRetrieved");
                }
            }
            State::ClockAdjusted(inner) => {
                let ClockAdjusted { instant } = inner;
                if now.duration_since(*instant) >= ClockAdjusted::DURATION {
                    self.state =
                        State::PhaseCorrectHalted(inner.to_phase_correct_halted(&self.ntp_adjtime));
                } else {
                    trace!("No state transition expected from ClockAdjusted");
                }
            }
            State::PhaseCorrectHalted(inner) => {
                let PhaseCorrectHalted { instant } = inner;
                if now.duration_since(*instant) >= PhaseCorrectHalted::DURATION {
                    self.state =
                        State::SnapshotRetrieved(inner.to_snapshot_retrieved(&self.ntp_adjtime));
                } else {
                    trace!("No state transition expected from PhaseCorrectHalted");
                }
            }
            State::SnapshotRetrieved(inner) => {
                let SnapshotRetrieved { instant, snapshot } = inner;
                if now.duration_since(*instant) >= SnapshotRetrieved::DURATION {
                    self.state = State::ClockAdjusted(inner.to_clock_adjusted(
                        &self.ntp_adjtime,
                        clock_params,
                        snapshot,
                    ));
                } else {
                    trace!("No state transition expected from SnapshotRetrieved");
                }
            }
        }
    }
}

#[cfg(test)]
mod test {
    use crate::daemon::{
        clock_state::clock_adjust::{ntp_adjtime::MockNtpAdjTime, state_machine::ClockSnapshot},
        event::SystemClockMeasurement,
        time::{
            Instant, TscCount,
            timex::Timex,
            tsc::{Frequency, Period},
        },
    };
    use libc::TIME_ERROR;
    use mockall::predicate::eq;
    use rstest::rstest;

    use super::*;

    fn test_clock_parameters() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(0),
            period: Period::from_frequency(Frequency::from_hz(1_000_000.0)),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period_max_error: Period::from_seconds(0.0),
        }
    }

    fn test_clock_snapshot() -> ClockSnapshot {
        ClockSnapshot {
            system_clock: SystemClockMeasurement {
                system_time: Instant::from_secs(0),
                tsc: TscCount::new(0),
            },
            kernel_state: Timex::retrieve(),
        }
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

    #[rstest]
    #[case::initialized_to_initial_phase_correct_halted(
        State::Initialized(Initialized),
        State::InitialPhaseCorrectHalted(InitialPhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now(),
        0, // not used
    )]
    #[case::disrupted_to_initial_phase_correct_halted(
        State::Disrupted(Disrupted),
        State::InitialPhaseCorrectHalted(InitialPhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now(),
        1,
    )]
    #[case::initial_phase_correct_halted_no_change_if_too_early(
        State::InitialPhaseCorrectHalted(InitialPhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        State::InitialPhaseCorrectHalted(InitialPhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now(),
        0,
    )]
    #[case::initial_phase_correct_halted_to_initial_snapshot_retrieved(
        State::InitialPhaseCorrectHalted(InitialPhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        State::InitialSnapshotRetrieved(InitialSnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        tokio::time::Instant::now() + InitialPhaseCorrectHalted::DURATION,
        1,
    )]
    #[case::initial_snapshot_retrieved_no_change_if_too_early(
        State::InitialSnapshotRetrieved(InitialSnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        State::InitialSnapshotRetrieved(InitialSnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        tokio::time::Instant::now(),
        0,
    )]
    #[case::initial_snapshot_retrieved_to_clock_adjusted(
        State::InitialSnapshotRetrieved(InitialSnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        State::ClockAdjusted(ClockAdjusted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now() + InitialSnapshotRetrieved::DURATION,
        2,
    )]
    #[case::clock_adjusted_no_change_if_too_early(
        State::ClockAdjusted(ClockAdjusted { instant: tokio::time::Instant::now() }),
        State::ClockAdjusted(ClockAdjusted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now(),
        0,
    )]
    #[case::clock_adjusted_to_phase_correct_halted(
        State::ClockAdjusted(ClockAdjusted { instant: tokio::time::Instant::now() }),
        State::PhaseCorrectHalted(PhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now() + ClockAdjusted::DURATION,
        1,
    )]
    #[case::phase_correct_halted_no_change_if_too_early(
        State::PhaseCorrectHalted(PhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        State::PhaseCorrectHalted(PhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now(),
        0,
    )]
    #[case::initial_phase_correct_halted_to_snapshot_retrieved(
        State::PhaseCorrectHalted(PhaseCorrectHalted { instant: tokio::time::Instant::now() }),
        State::SnapshotRetrieved(SnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        tokio::time::Instant::now() + PhaseCorrectHalted::DURATION,
        1,
    )]
    #[case::snapshot_retrieved_no_change_if_too_early(
        State::SnapshotRetrieved(SnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        State::SnapshotRetrieved(SnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        tokio::time::Instant::now(),
        0,
    )]
    #[case::snapshot_retrieved_to_clock_adjusted(
        State::SnapshotRetrieved(SnapshotRetrieved { instant: tokio::time::Instant::now(), snapshot: test_clock_snapshot() }),
        State::ClockAdjusted(ClockAdjusted { instant: tokio::time::Instant::now() }),
        tokio::time::Instant::now() + SnapshotRetrieved::DURATION,
        2,
    )]
    #[tokio::test(start_paused = true)]
    async fn handle_event(
        #[case] initial_state: State,
        #[case] final_state: State,
        #[case] instant: tokio::time::Instant,
        #[case] ntp_adjtime_call_count: usize,
    ) {
        let mut mock_ntp_adj_time = MockNtpAdjTime::new();
        // Silly thing, but the only case we ever expect to not return TIME_OK (0) is
        // when initializing, and stepping the clock (expect TIME_ERROR in that case).
        if let State::Initialized(_) = initial_state {
            let ntp_adjtime_returns = [0, TIME_ERROR];
            let mut ntp_adjtime_call_count = 0;
            mock_ntp_adj_time.expect_ntp_adjtime().returning(move |_| {
                let ret = ntp_adjtime_returns[ntp_adjtime_call_count];
                ntp_adjtime_call_count += 1;
                ret
            });
        } else {
            mock_ntp_adj_time
                .expect_ntp_adjtime()
                .times(ntp_adjtime_call_count)
                .return_const(0);
        }
        let mut clock_adjuster = ClockAdjuster {
            state: initial_state,
            ntp_adjtime: mock_ntp_adj_time,
        };
        clock_adjuster.handle_event(instant, &test_clock_parameters());
        match final_state {
            State::Disrupted(_) => assert!(matches!(clock_adjuster.state, State::Disrupted(_))),
            State::Initialized(_) => assert!(matches!(clock_adjuster.state, State::Initialized(_))),
            State::InitialPhaseCorrectHalted(_) => assert!(matches!(
                clock_adjuster.state,
                State::InitialPhaseCorrectHalted(_)
            )),
            State::InitialSnapshotRetrieved(_) => assert!(matches!(
                clock_adjuster.state,
                State::InitialSnapshotRetrieved(_)
            )),
            State::ClockAdjusted(_) => {
                assert!(matches!(clock_adjuster.state, State::ClockAdjusted(_)))
            }
            State::PhaseCorrectHalted(_) => {
                assert!(matches!(clock_adjuster.state, State::PhaseCorrectHalted(_)))
            }
            State::SnapshotRetrieved(_) => {
                assert!(matches!(clock_adjuster.state, State::SnapshotRetrieved(_)))
            }
        }
    }
}
