//! Adjust system clock and clockbound shared memory
pub mod clock_adjust;
pub mod clock_state_writer;

use tracing::info;

use crate::daemon::MAX_DISPERSION_GROWTH_PPB;
use crate::daemon::async_ring_buffer::Receiver;
use crate::daemon::clock_parameters::ClockParameters;
#[cfg(not(feature = "test-side-by-side"))]
use crate::daemon::clock_state::clock_adjust::KAPIClockAdjuster;
#[cfg(feature = "test-side-by-side")]
use crate::daemon::clock_state::clock_adjust::NoopClockAdjuster;
use crate::daemon::clock_state::clock_adjust::{ClockAdjust, ClockAdjuster};
use crate::daemon::clock_state::clock_state_writer::ClockStateWriter;
use crate::daemon::clock_state::clock_state_writer::{ClockStateWrite, SafeShmWriter};
use crate::daemon::io::tsc::ReadTscImpl;
use crate::daemon::time::ClockExt;
use crate::daemon::time::clocks::{ClockBound, RealTime};
use crate::shm::{CLOCKBOUND_SHM_DEFAULT_PATH, ShmWriter};

/// The whole `ClockState` component struct.
/// This encompasses both `ClockAdjust` component which interfaces
/// with the `CLOCK_REALTIME` kernel clock to synchronize it with `ClockBound` estimate
/// of UTC (`ClockBound` clock), and `ClockStateWriter` which manages writing
/// the `ClockErrorBound` to SHM segment for the client to read.
pub(crate) struct ClockState {
    state_writer: Box<dyn ClockStateWrite>,
    clock_adjuster: Box<dyn ClockAdjust>,
    clock_parameters: Option<ClockParameters>,
    interval: tokio::time::Interval,
    clock_params_receiver: Receiver<ClockParameters>,
}

impl ClockState {
    pub fn new(
        clock_state_writer: Box<dyn ClockStateWrite>,
        clock_adjuster: Box<dyn ClockAdjust>,
        clock_params_receiver: Receiver<ClockParameters>,
    ) -> Self {
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
        Self {
            state_writer: clock_state_writer,
            clock_adjuster,
            interval,
            clock_params_receiver,
            clock_parameters: None,
        }
    }

    pub fn construct(clock_params_receiver: Receiver<ClockParameters>) -> Self {
        let shm_writer = ShmWriter::new(std::path::Path::new(CLOCKBOUND_SHM_DEFAULT_PATH)).unwrap();
        let safe_shm_writer = SafeShmWriter::new(shm_writer);
        let clock_state_writer: ClockStateWriter<SafeShmWriter> = ClockStateWriter::builder()
            .clock_disruption_support_enabled(true)
            .shm_writer(safe_shm_writer)
            .max_drift_ppb(MAX_DISPERSION_GROWTH_PPB)
            .disruption_marker(0)
            .build();
        #[cfg(not(feature = "test-side-by-side"))]
        let clock_adjuster: ClockAdjuster<KAPIClockAdjuster> =
            ClockAdjuster::new(KAPIClockAdjuster);
        #[cfg(feature = "test-side-by-side")]
        let clock_adjuster: ClockAdjuster<NoopClockAdjuster> =
            ClockAdjuster::new(NoopClockAdjuster);

        Self::new(
            Box::new(clock_state_writer),
            Box::new(clock_adjuster),
            clock_params_receiver,
        )
    }

    pub async fn run(&mut self) {
        info!("Starting run for ClockState");
        loop {
            tokio::select! {
                now = self.interval.tick() => {
                    self.handle_tick(now);
                },
                params = self.clock_params_receiver.recv() => {
                    self.handle_clock_parameters(params.unwrap()); // todo fixme
                }
            }
        }
    }

    fn handle_tick(&mut self, now: tokio::time::Instant) {
        if let Some(parameters) = &self.clock_parameters {
            self.clock_adjuster.handle_clock_parameters(now, parameters);
            let clock_status = self.clock_adjuster.get_clock_realtime_status();
            // FIXME: Initializing behavior of ClockStateWriter should have us write
            // the initial clock status as unknown to SHM segment. Else, the clock might be adjusted
            // by ClockAdjuster on its initial state, while the SHM isn't updated with that info..
            let clockbound_clock = ClockBound::new(parameters.clone(), ReadTscImpl);
            // TODO: implement multiple attempts in case of latency increase
            let clock_realtime_offset_and_rtt = clockbound_clock.get_offset_and_rtt(&RealTime);
            self.state_writer.handle_clock_parameters(
                parameters,
                clock_status,
                clock_realtime_offset_and_rtt,
            );
        }
    }

    /// Handles `ClockParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Panics
    /// If the `ClockAdjuster` fails, e.g. an invalid value was supplied to `ntp_adjtime`, or
    /// insufficient permissions to adjust the clock.
    pub(crate) fn handle_clock_parameters(
        &mut self,
        // This is needed to tell ClockAdjust what frequency to use.
        clock_parameters: ClockParameters,
    ) {
        self.clock_parameters = Some(clock_parameters);
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
            state_writer: clock_state_writer,
            clock_params_receiver,
            interval: _,
            clock_parameters,
        } = self;
        *clock_parameters = None;
        clock_params_receiver.handle_disruption();
        clock_adjuster.handle_disruption(new_disruption_marker);
        clock_state_writer.handle_disruption(new_disruption_marker);
        tracing::info!("Handled clock disruption event");
    }
}

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;

    use crate::{
        daemon::{
            async_ring_buffer,
            clock_state::{clock_adjust::MockClockAdjust, clock_state_writer::MockClockStateWrite},
            time::{Duration, Instant, TscCount, tsc::Period},
        },
        shm::ClockStatus,
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

    #[tokio::test]
    async fn handle_clock_parameters() {
        let clock_parameters = get_sample_clock_parameters();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .never()
            .return_const(());

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .never()
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
        );
        assert_eq!(clock_state.clock_parameters, None);
        clock_state.handle_clock_parameters(clock_parameters.clone());
        assert_eq!(clock_state.clock_parameters, Some(clock_parameters));
    }

    #[tokio::test]
    async fn handle_disruption() {
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
        let (_tx, rx) = async_ring_buffer::create(1);
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
        );
        clock_state.handle_disruption(disruption_marker);
    }

    #[tokio::test]
    async fn handle_tick_no_parameters() {
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .never()
            .return_const(());

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .never()
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
        );
        clock_state.clock_parameters = None;
        clock_state.handle_tick(tokio::time::Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn handle_tick_with_parameters() {
        let mut sequence = mockall::Sequence::new();
        let expected_clock_status = ClockStatus::Synchronized;
        let expected_clock_params = get_sample_clock_parameters();
        let expected_instant = tokio::time::Instant::now();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();

        let expected_clock_params_clone = expected_clock_params.clone();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .withf(move |actual_instant, actual_clock_params| {
                *actual_instant == expected_instant
                    && *actual_clock_params == expected_clock_params_clone
            })
            .in_sequence(&mut sequence)
            .return_const(());
        mock_clock_adjuster
            .expect_get_clock_realtime_status()
            .once()
            .in_sequence(&mut sequence)
            .return_const(expected_clock_status);

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        let expected_clock_params_clone = expected_clock_params.clone();
        mock_clock_state_writer
            .expect_handle_clock_parameters()
            .once()
            .withf(
                move |actual_clock_params, actual_clock_status, _offset_and_rtt| {
                    *actual_clock_params == expected_clock_params_clone
                        && *actual_clock_status == expected_clock_status
                },
            )
            .in_sequence(&mut sequence)
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
        );
        clock_state.clock_parameters = Some(expected_clock_params);
        clock_state.handle_tick(expected_instant);
    }
}
