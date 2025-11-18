//! Adjust system clock and clockbound shared memory
pub mod clock_adjust;
pub mod clock_state_writer;

use std::path::Path;
use tokio_util::sync::CancellationToken;
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
use crate::daemon::subscriber::CLOCK_METRICS_TARGET;
use crate::daemon::time::ClockExt;
use crate::daemon::time::clocks::{ClockBound, MonotonicRaw, RealTime};
use crate::shm::{
    CLOCKBOUND_SHM_DEFAULT_PATH_V0, CLOCKBOUND_SHM_DEFAULT_PATH_V1, ClockErrorBoundLayoutVersion,
    ShmWriter,
};

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
    cancellation_token: CancellationToken,
}

impl ClockState {
    pub fn new(
        clock_state_writer: Box<dyn ClockStateWrite>,
        clock_adjuster: Box<dyn ClockAdjust>,
        clock_params_receiver: Receiver<ClockParameters>,
        cancellation_token: CancellationToken,
    ) -> Self {
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
        Self {
            state_writer: clock_state_writer,
            clock_adjuster,
            interval,
            clock_params_receiver,
            clock_parameters: None,
            cancellation_token,
        }
    }

    pub fn construct(
        clock_params_receiver: Receiver<ClockParameters>,
        cancellation_token: CancellationToken,
    ) -> Self {
        // Build two writers, each writing to a specific shared memory segment path.
        //
        // FIXME: given these path are const strings, would be worth looking into moving the
        // creation of these writers to the ClockStateWriter::new() method.
        //
        let shm_writer_0 = ShmWriter::new(
            Path::new(CLOCKBOUND_SHM_DEFAULT_PATH_V0),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .unwrap();
        let safe_shm_writer_0 = SafeShmWriter::new(shm_writer_0);

        let shm_writer_1 = ShmWriter::new(
            Path::new(CLOCKBOUND_SHM_DEFAULT_PATH_V1),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .unwrap();
        let safe_shm_writer_1 = SafeShmWriter::new(shm_writer_1);

        let clock_state_writer: ClockStateWriter<SafeShmWriter> = ClockStateWriter::builder()
            .clock_disruption_support_enabled(true)
            .shm_writer_0(safe_shm_writer_0)
            .shm_writer_1(safe_shm_writer_1)
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
            cancellation_token,
        )
    }

    pub async fn run(&mut self) {
        let mut clock_offset_metric_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(1));
        info!("Starting run for ClockState");
        loop {
            tokio::select! {
                _ = clock_offset_metric_interval.tick() => {
                    self.emit_clock_offsets();
                },
                now = self.interval.tick() => {
                    self.handle_tick(now);
                },
                params = self.clock_params_receiver.recv() => {
                    self.handle_clock_parameters(params.unwrap()); // todo fixme
                },
                () = self.cancellation_token.cancelled() => {
                    // nothing fancy for now. just exit
                    // TODO: we may want to clean-up SHM here.
                    info!("Received shutdown signal. Exiting.");
                    break;
                },
            }
        }
        info!("ClockState runner exiting.");
    }

    fn emit_clock_offsets(&self) {
        let realtime_to_monotonic_raw = RealTime.get_offset_and_rtt(&MonotonicRaw);
        tracing::info!(
            target: CLOCK_METRICS_TARGET,
            realtime_to_monotonic_raw = serde_json::to_string(&realtime_to_monotonic_raw).unwrap(),
            "metrics"
        );
        if let Some(parameters) = &self.clock_parameters {
            let clockbound_clock = ClockBound::new(parameters.clone(), ReadTscImpl);
            let clockbound_to_realtime = clockbound_clock.get_offset_and_rtt(&RealTime);
            let clockbound_to_monotonic_raw = clockbound_clock.get_offset_and_rtt(&MonotonicRaw);
            tracing::info!(
                target: CLOCK_METRICS_TARGET,
                clockbound_to_realtime = serde_json::to_string(&clockbound_to_realtime).unwrap(),
                "metrics"
            );
            tracing::info!(
                target: CLOCK_METRICS_TARGET,
                clockbound_to_monotonic_raw = serde_json::to_string(&clockbound_to_monotonic_raw).unwrap(),
                "metrics"
            );
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
            cancellation_token: _,
        } = self;

        // Update the clock status on the shared memory segments
        if let Some(params) = clock_parameters {
            clock_state_writer.handle_disruption(params, new_disruption_marker);
        }

        *clock_parameters = None;
        clock_params_receiver.handle_disruption();
        clock_adjuster.handle_disruption(new_disruption_marker);

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
        let cancellation_token = CancellationToken::new();
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
            cancellation_token,
        );
        assert_eq!(clock_state.clock_parameters, None);
        clock_state.handle_clock_parameters(clock_parameters.clone());
        assert_eq!(clock_state.clock_parameters, Some(clock_parameters));
    }

    #[tokio::test]
    async fn handle_disruption() {
        let disruption_marker = 123;
        let cancellation_token = CancellationToken::new();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(1000),
            time: Instant::from_nanos(1000),
            clock_error_bound: Duration::from_nanos(1000),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
        };
        let expected_clock_parameters = clock_parameters.clone();
        mock_clock_adjuster
            .expect_handle_disruption()
            .once()
            .with(eq(disruption_marker))
            .return_const(());
        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_disruption()
            .once()
            .withf(move |param: &ClockParameters, marker: &u64| {
                *param == expected_clock_parameters && *marker == 123
            })
            .return_const(());
        let (_tx, rx) = async_ring_buffer::create(1);
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            cancellation_token,
        );
        clock_state.clock_parameters = Some(clock_parameters);

        clock_state.handle_disruption(disruption_marker);
    }

    #[tokio::test]
    async fn handle_tick_no_parameters() {
        let cancellation_token = CancellationToken::new();
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
            cancellation_token,
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
        let cancellation_token = CancellationToken::new();
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
            cancellation_token,
        );
        clock_state.clock_parameters = Some(expected_clock_params);
        clock_state.handle_tick(expected_instant);
    }
}
