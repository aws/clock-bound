//! Adjust system clock and clockbound shared memory
pub mod adjust_data;
pub mod clock_adjust;
pub mod clock_state_writer;

use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::daemon::MAX_DISPERSION_GROWTH_PPB;
use crate::daemon::async_ring_buffer::Receiver;
use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_state::adjust_data::ClockAdjustData;
#[cfg(not(feature = "test-side-by-side"))]
use crate::daemon::clock_state::clock_adjust::KAPIClockAdjuster;
#[cfg(feature = "test-side-by-side")]
use crate::daemon::clock_state::clock_adjust::NoopClockAdjuster;
use crate::daemon::clock_state::clock_adjust::{ClockAdjust, ClockAdjuster, ClockCorrection};
use crate::daemon::clock_state::clock_state_writer::ClockStateWriter;
use crate::daemon::clock_state::clock_state_writer::{ClockStateWrite, SafeShmWriter};
use crate::daemon::clock_sync_algorithm::SyncParameters;
use crate::daemon::io::ClockDisruptionEvent;
use crate::daemon::io::tsc::ReadTscImpl;
use crate::daemon::io::vmclock::{State as VMClockState, VMClockParams};
use crate::daemon::logging::CLOCK_METRICS_TARGET;
use crate::daemon::logging::synchronization::{
    log_clock_status_change, log_sync_snapshot, log_system_clock_step, log_system_clock_summary,
};
use crate::daemon::time::clocks::{ClockBound, MonotonicCoarse, MonotonicRaw, RealTime};
use crate::daemon::time::{Clock, ClockExt, Duration};
use crate::shm::{
    CLOCKBOUND_SHM_DEFAULT_PATH_V0, CLOCKBOUND_SHM_DEFAULT_PATH_V1, ClockErrorBoundLayoutVersion,
    ClockStatus, ShmWriter,
};

const FREE_RUNNING_GRACE_PERIOD: Duration = Duration::from_secs(60);

const CLOCK_ADJUST_SUMMARY_PERIOD_SECS: u64 = 600;

/// The whole `ClockState` component struct.
/// This encompasses both `ClockAdjust` component which interfaces
/// with the `CLOCK_REALTIME` kernel clock to synchronize it with `ClockBound` estimate
/// of UTC (`ClockBound` clock), and `ClockStateWriter` which manages writing
/// the `ClockErrorBound` to SHM segment for the client to read.
pub(crate) struct ClockState {
    state_writer: Box<dyn ClockStateWrite>,
    clock_adjuster: Box<dyn ClockAdjust>,
    sync_parameters: Option<SyncParameters>,
    clock_status: ClockStatus,
    interval: tokio::time::Interval,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    clock_params_receiver: Receiver<SyncParameters>,
    cancellation_token: CancellationToken,
    vmclock_state: Option<Arc<Mutex<VMClockState>>>,
    adjust_data: ClockAdjustData,
}

impl ClockState {
    pub fn new(
        clock_state_writer: Box<dyn ClockStateWrite>,
        clock_adjuster: Box<dyn ClockAdjust>,
        clock_params_receiver: Receiver<SyncParameters>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        cancellation_token: CancellationToken,
        clock_status: ClockStatus,
        vmclock_state: Option<Arc<Mutex<VMClockState>>>,
    ) -> Self {
        let interval = tokio::time::interval(tokio::time::Duration::from_millis(100));
        Self {
            state_writer: clock_state_writer,
            clock_adjuster,
            interval,
            clock_params_receiver,
            clock_disruption_receiver,
            sync_parameters: None,
            clock_status,
            cancellation_token,
            vmclock_state,
            adjust_data: ClockAdjustData::default(),
        }
    }

    pub fn construct(
        clock_params_receiver: Receiver<SyncParameters>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        cancellation_token: CancellationToken,
        vmclock_params: Option<VMClockParams>,
        clock_status: ClockStatus,
    ) -> Self {
        // Clock disruption support is enabled whenever a VMClock source exists (i.e. we expected
        // one on this platform), even if it failed to initialize.
        let clock_disruption_support_enabled = vmclock_params.is_some();
        let disruption_marker = vmclock_params
            .as_ref()
            .map_or(0, |params| params.disruption_marker);
        let vmclock_state = vmclock_params.map(|params| params.shared_state);

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
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .shm_writer_0(safe_shm_writer_0)
            .shm_writer_1(safe_shm_writer_1)
            .max_drift_ppb(MAX_DISPERSION_GROWTH_PPB)
            .disruption_marker(disruption_marker)
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
            clock_disruption_receiver,
            cancellation_token,
            clock_status,
            vmclock_state,
        )
    }

    /// Returns `true` if a VMClock source exists but its shared state is currently `Failed`.
    ///
    /// In this case the `ClockStatus` written to SHM must be forced to `ClockStatus::Unknown`.
    fn is_vmclock_in_failed_state(&self) -> bool {
        self.vmclock_state
            .as_ref()
            .is_some_and(|state| *state.lock().unwrap() == VMClockState::Failed)
    }

    pub async fn run(&mut self) {
        let mut clock_offset_metric_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(1));
        let mut snapshot_interval = tokio::time::interval(tokio::time::Duration::from_secs(8));
        snapshot_interval.reset(); // delay the first tick by one period
        let mut clock_adjust_summary_interval = tokio::time::interval(
            tokio::time::Duration::from_secs(CLOCK_ADJUST_SUMMARY_PERIOD_SECS),
        );
        clock_adjust_summary_interval.reset();
        info!("Starting run for ClockState");
        // FIXME: This clears the SHM segment quite early, before we have
        // even received `ClockParameters` and started trying to adjust the clock -
        // it is overly cautious. We could hold off even longer before clearing things,
        // in case a previous `ClockBound` has written reliable SHM data.
        // We could consider to wait til we are about to step the clock, and set the
        // clock status to unknown right before then.
        self.state_writer.initialize_ceb_v2_shm();
        loop {
            tokio::select! {
                biased; // biased to ensure disruption is handled first when this happens
                Ok(()) = self.clock_disruption_receiver.changed() => {
                    self.handle_disruption();
                }
                source_params = self.clock_params_receiver.recv() => {
                    self.handle_sync_update(source_params.unwrap()); // todo fixme
                },
                _ = snapshot_interval.tick() => {
                    log_sync_snapshot(self.sync_parameters.as_ref(), self.clock_status);
                },
                _ = clock_adjust_summary_interval.tick() => {
                    self.flush_clock_adjust_summary();
                },
                _ = clock_offset_metric_interval.tick() => {
                    self.emit_clock_offsets();
                },
                now = self.interval.tick() => {
                    self.handle_tick(now);
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
        if let Some(sp) = &self.sync_parameters {
            let clockbound_clock = ClockBound::new(sp.clock_parameters.clone(), ReadTscImpl);
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

    /// Determines the `ClockStatus` to write to the SHM for `ClockErrorBoundV2`, which depends on the system
    /// clock underneath the hood.
    fn determine_clock_error_bound_v2_status(&self, parameters: &ClockParameters) -> ClockStatus {
        let mut clock_status = self.clock_adjuster.get_clock_realtime_status();
        // Check if the clock parameters have been stale for an extended time, if so then update to status `ClockStatus::FreeRunning`
        let time_since_parameters_updated = MonotonicCoarse.get_time() - parameters.as_of_monotonic;
        if clock_status == ClockStatus::Synchronized
            && time_since_parameters_updated >= FREE_RUNNING_GRACE_PERIOD
        {
            clock_status = ClockStatus::FreeRunning;
        }
        clock_status
    }

    /// Determines the `ClockStatus` to write to the SHM for `ClockErrorBoundV3`.
    /// The `ClockErrorBoundV3` does not directly rely on the kernel system clock.
    /// After a disruption, `ClockState` should have no `ClockParameters`, so we don't expect
    /// to have to return `ClockState::Disrupted` at all. Similar reasoning also applies for `ClockState::Unknown`
    /// not being covered here.
    ///
    /// If we have received any `ClockParameters` in the caller, we have some notion of a clock, and can say we're `Synchronized`, or
    /// if the `ClockParameters` are stale for `FREE_RUNNING_GRACE_PERIOD`, we may declare ourselves `FreeRunning`.
    fn determine_clock_error_bound_v3_status(clock_params_age: Duration) -> ClockStatus {
        if clock_params_age < FREE_RUNNING_GRACE_PERIOD {
            ClockStatus::Synchronized
        } else {
            ClockStatus::FreeRunning
        }
    }

    /// Assign `new` to `status`, logging the transition to the synchronization log.
    fn set_clock_status(status: &mut ClockStatus, new: ClockStatus) {
        let previous = *status;
        *status = new;
        if new != previous {
            log_clock_status_change(previous, new);
        }
    }

    fn handle_tick(&mut self, now: tokio::time::Instant) {
        // If a VMClock was expected but has failed, the clock status is untrustworthy and both SHM
        // segments must report `ClockStatus::Unknown`.
        let vmclock_failed = self.is_vmclock_in_failed_state();

        if let Some(sp) = &self.sync_parameters {
            let parameters = &sp.clock_parameters;
            match self.clock_adjuster.handle_clock_parameters(now, parameters) {
                ClockCorrection::Step { step_ns } => log_system_clock_step(step_ns),
                ClockCorrection::Smooth { correction_ns } => {
                    self.adjust_data.record(correction_ns);
                }
            }

            // Handle SHM1 (`ClockErrorboundV3`) first
            let clockbound_clock = ClockBound::new(parameters.clone(), ReadTscImpl);
            // Get the age of the `ClockParameters`, to determine if we're Synchronized or FreeRunning.
            // Inherently, we can't be `Disrupted` anymore if we've gotten any `ClockParameters`.
            let clock_params_age_v3 = clockbound_clock.get_time() - parameters.time;
            let mut clock_status_v3 =
                ClockState::determine_clock_error_bound_v3_status(clock_params_age_v3);
            if vmclock_failed {
                clock_status_v3 = ClockStatus::Unknown;
            }

            // Log clock status transitions and update shared state
            Self::set_clock_status(&mut self.clock_status, clock_status_v3);

            self.state_writer
                .handle_clock_parameters_shm1(parameters, clock_status_v3);

            // Handle SHM0 (`ClockErrorboundV2`) after SHM1.
            let mut clock_status_v2 = self.determine_clock_error_bound_v2_status(parameters);
            if vmclock_failed {
                clock_status_v2 = ClockStatus::Unknown;
            }
            self.state_writer
                .handle_clock_parameters_shm0(parameters, clock_status_v2);
        }
    }

    /// Emit a `system_clock_summary` for the smooth phase corrections accumulated
    /// over the current window and reset the window.
    fn flush_clock_adjust_summary(&mut self) {
        log_system_clock_summary(CLOCK_ADJUST_SUMMARY_PERIOD_SECS, &self.adjust_data);
        self.adjust_data = ClockAdjustData::default();
    }

    /// Handles `SyncParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// # Panics
    /// If the `ClockAdjuster` fails, e.g. an invalid value was supplied to `ntp_adjtime`, or
    /// insufficient permissions to adjust the clock.
    pub(crate) fn handle_sync_update(&mut self, sync_parameters: SyncParameters) {
        self.sync_parameters = Some(sync_parameters);
    }

    /// Handle a clock disruption event
    ///
    /// Call this function after the system detects a VMClock disruption event.
    ///
    /// It will go through and clear the state (like startup).
    pub fn handle_disruption(&mut self) {
        // Use the destructure pattern to get a mutable reference to each item.
        //
        // This makes it a compilation error if we add a new field this Self without handling it here
        let Self {
            clock_adjuster,
            state_writer: clock_state_writer,
            clock_params_receiver,
            clock_disruption_receiver,
            interval: _,
            sync_parameters,
            clock_status,
            cancellation_token: _,
            vmclock_state: _,
            adjust_data: _,
        } = self;

        let val = clock_disruption_receiver.borrow_and_update().clone();
        if let Some(disruption_marker) = val.disruption_marker {
            // Update the clock status on the shared memory segments
            if let Some(sp) = sync_parameters {
                clock_state_writer.handle_disruption(&sp.clock_parameters, disruption_marker);
            }

            *sync_parameters = None;
            clock_params_receiver.handle_disruption();
            clock_adjuster.handle_disruption(disruption_marker);

            // Log clock status transition to Disrupted and update shared state
            Self::set_clock_status(clock_status, ClockStatus::Disrupted);

            tracing::info!("Handled clock disruption event");
        }
    }
}

#[cfg(test)]
mod tests {
    use mockall::predicate::eq;
    use rstest::rstest;

    use crate::{
        daemon::{
            async_ring_buffer,
            clock_state::{clock_adjust::MockClockAdjust, clock_state_writer::MockClockStateWrite},
            clock_sync_algorithm::SourceInfo,
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
            as_of_monotonic: Instant::new(0),
        }
    }

    #[tokio::test]
    async fn handle_clock_parameters() {
        let clock_parameters = get_sample_clock_parameters();
        let cancellation_token = CancellationToken::new();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster.expect_handle_clock_parameters().never();

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm0()
            .never();

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());

        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            None,
        );
        assert_eq!(clock_state.sync_parameters, None);
        clock_state.handle_sync_update(SyncParameters {
            clock_parameters: clock_parameters.clone(),
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
        assert_eq!(
            clock_state.sync_parameters,
            Some(SyncParameters {
                clock_parameters,
                source_info: SourceInfo::Phc("/dev/ptp0".into()),
                selected_at: Instant::new(0),
                selected_at_clock_error_bound: Duration::new(0),
            })
        );
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
            as_of_monotonic: Instant::from_nanos(1000),
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
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            None,
        );
        clock_state.sync_parameters = Some(SyncParameters {
            clock_parameters,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });

        clock_disruption_sender
            .send(ClockDisruptionEvent {
                disruption_marker: Some(disruption_marker),
            })
            .unwrap();
        clock_state.handle_disruption();
    }

    #[tokio::test]
    async fn handle_tick_no_parameters() {
        let cancellation_token = CancellationToken::new();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster.expect_handle_clock_parameters().never();

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm0()
            .never();

        mock_clock_state_writer
            .expect_handle_clock_parameters_shm1()
            .never();

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            None,
        );
        clock_state.sync_parameters = None;
        clock_state.handle_tick(tokio::time::Instant::now());
    }

    #[tokio::test(start_paused = true)]
    async fn handle_tick_with_parameters() {
        let mut sequence = mockall::Sequence::new();
        let expected_clock_error_bound_v2_status = ClockStatus::Synchronized;
        let expected_clock_error_bound_v3_status = ClockStatus::Synchronized;
        let mut expected_clock_params = get_sample_clock_parameters();
        // If we don't overwrite `as_of_monotonic`, the default from `get_sample_clock_parameters`
        // has it at `Instant::new(0)` which would cause us to declare the clock as `FreeRunning`
        // since it's likely far in the past.
        expected_clock_params.as_of_monotonic = MonotonicCoarse.get_time();

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
            .return_const(ClockCorrection::Smooth { correction_ns: 0 });

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        let expected_clock_params_clone = expected_clock_params.clone();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm1()
            .once()
            .withf(move |actual_clock_params, actual_clock_status| {
                *actual_clock_params == expected_clock_params_clone
                    && *actual_clock_status == expected_clock_error_bound_v3_status
            })
            .in_sequence(&mut sequence)
            .return_const(());
        mock_clock_adjuster
            .expect_get_clock_realtime_status()
            .once()
            .in_sequence(&mut sequence)
            // Return synchronized state from `ClockAdjuster` so that
            // we may test aging the clock params to get `ClockStatus::FreeRunning`
            .return_const(ClockStatus::Synchronized);
        let expected_clock_params_clone = expected_clock_params.clone();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm0()
            .once()
            .withf(move |actual_clock_params, actual_clock_status| {
                *actual_clock_params == expected_clock_params_clone
                    && *actual_clock_status == expected_clock_error_bound_v2_status
            })
            .in_sequence(&mut sequence)
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            None,
        );
        clock_state.sync_parameters = Some(SyncParameters {
            clock_parameters: expected_clock_params,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
        clock_state.handle_tick(expected_instant);
    }

    /// When a VMClock is present but in the `Failed` state, both SHM segments must be written with
    /// `ClockStatus::Unknown`, regardless of what the clock parameters would otherwise imply.
    #[tokio::test(start_paused = true)]
    async fn handle_tick_vmclock_failed_forces_unknown() {
        let mut expected_clock_params = get_sample_clock_parameters();
        expected_clock_params.as_of_monotonic = MonotonicCoarse.get_time();
        let expected_instant = tokio::time::Instant::now();
        let cancellation_token = CancellationToken::new();

        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .return_const(ClockCorrection::Smooth { correction_ns: 0 });
        // `determine_clock_error_bound_v2_status` still queries the adjuster; its result is
        // overridden to Unknown afterwards because the VMClock is Failed.
        mock_clock_adjuster
            .expect_get_clock_realtime_status()
            .once()
            .return_const(ClockStatus::Synchronized);

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm1()
            .once()
            .withf(|_clock_params, clock_status| *clock_status == ClockStatus::Unknown)
            .return_const(());
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm0()
            .once()
            .withf(|_clock_params, clock_status| *clock_status == ClockStatus::Unknown)
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let vmclock_state = Some(Arc::new(Mutex::new(VMClockState::Failed)));
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            vmclock_state,
        );
        clock_state.sync_parameters = Some(SyncParameters {
            clock_parameters: expected_clock_params,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
        clock_state.handle_tick(expected_instant);
    }

    /// When no VMClock is present (`None`) or one is present and `Running`, `handle_tick` behaves
    /// nominally (no forced Unknown): a fresh set of clock parameters yields `Synchronized`.
    #[rstest]
    #[case::vmclock_none(None)]
    #[case::vmclock_running(Some(Arc::new(Mutex::new(VMClockState::Running))))]
    #[tokio::test(start_paused = true)]
    async fn handle_tick_nominal(#[case] vmclock_state: Option<Arc<Mutex<VMClockState>>>) {
        let mut expected_clock_params = get_sample_clock_parameters();
        expected_clock_params.as_of_monotonic = MonotonicCoarse.get_time();
        let expected_instant = tokio::time::Instant::now();
        let cancellation_token = CancellationToken::new();

        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        mock_clock_adjuster
            .expect_handle_clock_parameters()
            .once()
            .return_const(ClockCorrection::Smooth { correction_ns: 0 });
        mock_clock_adjuster
            .expect_get_clock_realtime_status()
            .once()
            .return_const(ClockStatus::Synchronized);

        let mut mock_clock_state_writer: MockClockStateWrite = MockClockStateWrite::new();
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm1()
            .once()
            .withf(|_clock_params, clock_status| *clock_status == ClockStatus::Synchronized)
            .return_const(());
        mock_clock_state_writer
            .expect_handle_clock_parameters_shm0()
            .once()
            .withf(|_clock_params, clock_status| *clock_status == ClockStatus::Synchronized)
            .return_const(());

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            Box::new(mock_clock_state_writer),
            Box::new(mock_clock_adjuster),
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            vmclock_state,
        );
        clock_state.sync_parameters = Some(SyncParameters {
            clock_parameters: expected_clock_params,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
        clock_state.handle_tick(expected_instant);
    }

    /// `vmclock_failed` reflects the shared state: `None` and `Some(Running)` are not failed,
    /// `Some(Failed)` is.
    #[tokio::test]
    async fn vmclock_failed_reflects_shared_state() {
        let cancellation_token = CancellationToken::new();
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());

        let build = |vmclock_state: Option<Arc<Mutex<VMClockState>>>| {
            let (_, rx) = async_ring_buffer::create(1);
            ClockState::new(
                Box::new(MockClockStateWrite::new()),
                Box::new(MockClockAdjust::new()),
                rx,
                clock_disruption_receiver.clone(),
                cancellation_token.clone(),
                ClockStatus::Unknown,
                vmclock_state,
            )
        };

        assert!(!build(None).is_vmclock_in_failed_state());
        assert!(
            !build(Some(Arc::new(Mutex::new(VMClockState::Running)))).is_vmclock_in_failed_state()
        );
        assert!(
            build(Some(Arc::new(Mutex::new(VMClockState::Failed)))).is_vmclock_in_failed_state()
        );
    }

    #[rstest]
    #[case::synchronized_stays_synchronized_params_0sec_old(
        Duration::from_secs(0),
        ClockStatus::Synchronized,
        ClockStatus::Synchronized
    )]
    #[case::synchronized_stays_synchronized_params_30sec_old(
        Duration::from_secs(30),
        ClockStatus::Synchronized,
        ClockStatus::Synchronized
    )]
    #[case::synchronized_goes_freerunning_params_60sec_old(
        Duration::from_secs(60),
        ClockStatus::Synchronized,
        ClockStatus::FreeRunning
    )]
    #[case::synchronized_goes_freerunning_params_90sec_old(
        Duration::from_secs(90),
        ClockStatus::Synchronized,
        ClockStatus::FreeRunning
    )]
    #[case::unknown_stays_unknown_params_0sec_old(
        Duration::from_secs(0),
        ClockStatus::Unknown,
        ClockStatus::Unknown
    )]
    #[case::unknown_stays_unknown_params_90sec_old(
        Duration::from_secs(90),
        ClockStatus::Unknown,
        ClockStatus::Unknown
    )]
    #[case::disrupted_stays_disrupted_params_0sec_old(
        Duration::from_secs(0),
        ClockStatus::Disrupted,
        ClockStatus::Disrupted
    )]
    #[case::disrupted_stays_disrupted_params_90sec_old(
        Duration::from_secs(90),
        ClockStatus::Disrupted,
        ClockStatus::Disrupted
    )]
    #[tokio::test]
    async fn determine_clock_error_bound_v2_status(
        #[case] clock_params_age: Duration,
        #[case] clock_adjust_status: ClockStatus,
        #[case] expected_clock_error_bound_v2_status: ClockStatus,
    ) {
        let cancellation_token = CancellationToken::new();
        let mut mock_clock_adjuster: MockClockAdjust = MockClockAdjust::new();
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        mock_clock_adjuster
            .expect_get_clock_realtime_status()
            .once()
            .return_const(clock_adjust_status);
        let clock_state = ClockState::new(
            Box::new(MockClockStateWrite::new()),
            Box::new(mock_clock_adjuster),
            async_ring_buffer::create(1).1,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            None,
        );

        let mut clock_parameters = get_sample_clock_parameters();
        // Adjust the time reported on clock params to be `clock_params_age` old
        // For v2, we age clock params based on `CLOCK_MONOTONIC_COARSE`, so use that here
        clock_parameters.as_of_monotonic = MonotonicCoarse.get_time() - clock_params_age;
        let res = clock_state.determine_clock_error_bound_v2_status(&clock_parameters);
        assert_eq!(res, expected_clock_error_bound_v2_status);
    }

    #[rstest]
    #[case::synchronized_params_0sec_old(Duration::from_secs(0), ClockStatus::Synchronized)]
    #[case::synchronized_params_30sec_old(Duration::from_secs(30), ClockStatus::Synchronized)]
    #[case::freerunning_params_60sec_old(Duration::from_secs(60), ClockStatus::FreeRunning)]
    #[case::freerunning_params_90sec_old(Duration::from_secs(90), ClockStatus::FreeRunning)]
    fn determine_clock_error_bound_v3_status(
        #[case] clock_params_age: Duration,
        #[case] expected_clock_error_bound_v3_status: ClockStatus,
    ) {
        assert_eq!(
            ClockState::determine_clock_error_bound_v3_status(clock_params_age),
            expected_clock_error_bound_v3_status
        );
    }

    #[test]
    fn set_clock_status_logs_only_on_transition() {
        use crate::daemon::logging::StructuredLog;
        use crate::daemon::logging::test_support::with_layer;
        use serde_json::Value;

        let (output, _tmp) = with_layer(StructuredLog::Synchronization, "0.1.0-test", || {
            let mut status = ClockStatus::Unknown;
            // A real transition must be logged.
            ClockState::set_clock_status(&mut status, ClockStatus::Synchronized);
            // A same-value assignment must not be logged.
            ClockState::set_clock_status(&mut status, ClockStatus::Synchronized);
            assert_eq!(status, ClockStatus::Synchronized);
        });

        let lines: Vec<&str> = output.lines().collect();
        assert_eq!(
            lines.len(),
            1,
            "only the actual transition should be logged"
        );
        let entry: Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry["event"], "clock_status_change");
        assert_eq!(entry["clock_status_change"]["from"], "UNKNOWN");
        assert_eq!(entry["clock_status_change"]["to"], "SYNCHRONIZED");
    }
}
