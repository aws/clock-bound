//! Adjust system clock and clockbound shared memory
pub mod clock_adjust;
pub mod clock_state_writer;

use crate::daemon::clock_state::clock_adjust::{ClockAdjust, KAPIClockAdjuster};
use crate::daemon::clock_state::clock_state_writer::{ClockStateWrite, SafeShmWriter};
use crate::daemon::clock_state::{
    clock_adjust::ClockAdjuster, clock_state_writer::ClockStateWriter,
};
use crate::daemon::io::tsc::ReadTscImpl;
use crate::daemon::time::clocks::RealTime;
use crate::daemon::{
    clock_parameters::ClockParameters,
    time::{ClockExt, clocks::ClockBound},
};
use crate::shm::{CLOCKBOUND_SHM_DEFAULT_PATH, ShmWriter};

/// The whole `ClockState` component struct.
/// This encompasses both `ClockAdjust` component which interfaces
/// with the `CLOCK_REALTIME` kernel clock to synchronize it with `ClockBound` estimate
/// of UTC (`ClockBound` clock), and `ClockStateWriter` which manages writing
/// the `ClockErrorBound` to SHM segment for the client to read.
pub(crate) struct ClockState<A, S> {
    clock_state_writer: S,
    clock_adjuster: A,
}

#[cfg_attr(not(test), expect(unused))]
impl<A: ClockAdjust, S: ClockStateWrite> ClockState<A, S> {
    pub fn new(clock_state_writer: S, clock_adjust: A) -> Self {
        Self {
            clock_state_writer,
            clock_adjuster: clock_adjust,
        }
    }

    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Panics
    /// If the `ClockAdjuster` fails, e.g. an invalid value was supplied to `ntp_adjtime`, or
    /// insufficient permissions to adjust the clock.
    pub fn handle_clock_parameters(
        &mut self,
        // This is needed to tell ClockAdjust what frequency to use.
        clock_parameters: &ClockParameters,
    ) {
        let clockbound_clock = ClockBound::new(clock_parameters.clone(), ReadTscImpl);
        let clock_realtime_offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);
        // FIXME: this implementation is incorrect as is and will be fixed in upcoming commit
        // to integrate with the Daemon.
        self.clock_adjuster
            .handle_clock_parameters(tokio::time::Instant::now(), clock_parameters);
        self.clock_state_writer
            .handle_clock_parameters(clock_parameters, clock_realtime_offset_and_rtt);
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    pub fn handle_disruption(&mut self, new_disruption_marker: u64) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field this Self without handling it here
        let Self {
            clock_adjuster,
            clock_state_writer,
        } = self;

        clock_adjuster.handle_disruption(new_disruption_marker);
        clock_state_writer.handle_disruption(new_disruption_marker);
        tracing::info!("Handled clock disruption event");
    }
}

impl ClockState<ClockAdjuster<KAPIClockAdjuster>, ClockStateWriter<SafeShmWriter>> {
    #[cfg_attr(feature = "test-side-by-side", expect(unused))]
    pub fn construct() -> Self {
        let shm_writer = ShmWriter::new(std::path::Path::new(CLOCKBOUND_SHM_DEFAULT_PATH)).unwrap();
        let safe_shm_writer = SafeShmWriter::new(shm_writer);
        let clock_state_writer: ClockStateWriter<SafeShmWriter> = ClockStateWriter::builder()
            .clock_disruption_support_enabled(true)
            .shm_writer(safe_shm_writer)
            .max_drift_ppb(15_000)
            .disruption_marker(0)
            .build();
        let clock_adjuster: ClockAdjuster<KAPIClockAdjuster> =
            ClockAdjuster::new(KAPIClockAdjuster);
        Self::new(clock_state_writer, clock_adjuster)
    }
}

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;

    use crate::daemon::{
        clock_state::{clock_adjust::MockClockAdjust, clock_state_writer::MockClockStateWrite},
        time::{Duration, Instant, TscCount, inner::ClockOffsetAndRtt, instant::Utc, tsc::Period},
    };

    use super::*;

    fn get_sample_clock_parameters() -> ClockParameters {
        ClockParameters {
            tsc_count: TscCount::new(0),
            time: Instant::new(0),
            clock_error_bound: Duration::new(0),
            period: Period::from_seconds(0.0),
            period_max_error: Period::from_seconds(0.0),
        }
    }

    #[test]
    fn handle_clock_parameters() {
        let clock_parameters = get_sample_clock_parameters();
        let clock_parameters_clone = clock_parameters.clone();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .withf(
                move |_now: &tokio::time::Instant, clock_params: &ClockParameters| {
                    *clock_params == clock_parameters_clone
                },
            )
            .return_const(());

        let clock_parameters_clone = clock_parameters.clone();
        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .once()
            .withf(
                move |clock_params: &ClockParameters, _offset_and_rtt: &ClockOffsetAndRtt<Utc>| {
                    *clock_params == clock_parameters_clone
                },
            )
            .return_const(());

        let mut clock_state = ClockState::new(mock_clock_state_writer, mock_clock_adjuster);
        clock_state.handle_clock_parameters(&clock_parameters);
    }

    #[test]
    #[should_panic]
    fn handle_clock_parameters_clock_adjust_hard_failure() {
        let clock_parameters = get_sample_clock_parameters();
        let clock_parameters_clone = clock_parameters.clone();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .withf(
                move |_now: &tokio::time::Instant, clock_params: &ClockParameters| {
                    *clock_params == clock_parameters_clone
                },
            )
            .return_const(());

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .never();

        let mut clock_state = ClockState::new(mock_clock_state_writer, mock_clock_adjuster);
        clock_state.handle_clock_parameters(&clock_parameters);
    }

    #[test]
    fn handle_clock_parameters_clock_adjust_soft_failure() {
        let clock_parameters = get_sample_clock_parameters();
        let clock_parameters_clone = clock_parameters.clone();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .withf(
                move |_now: &tokio::time::Instant, clock_params: &ClockParameters| {
                    *clock_params == clock_parameters_clone
                },
            )
            .return_const(());

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .once()
            .return_const(());

        let mut clock_state = ClockState::new(mock_clock_state_writer, mock_clock_adjuster);
        clock_state.handle_clock_parameters(&clock_parameters);
    }

    #[test]
    fn handle_disruption() {
        let disruption_marker = 123;
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_disruption()
            .once()
            .with(eq(disruption_marker))
            .return_const(());
        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_disruption()
            .once()
            .with(eq(disruption_marker))
            .return_const(());
        let mut clock_state = ClockState::new(mock_clock_state_writer, mock_clock_adjuster);
        clock_state.handle_disruption(disruption_marker);
    }
}
