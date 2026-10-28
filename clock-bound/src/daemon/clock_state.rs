//! Adjust system clock and clockbound shared memory
pub mod adjust_data;
mod shm_writer;
pub mod system_clock_discipliner;

use shm_writer::{Shm0, Shm1};

use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::daemon::async_ring_buffer::Receiver;
use crate::daemon::clock_parameters::ClockParameters;
use crate::daemon::clock_state::adjust_data::ClockAdjustData;
use crate::daemon::clock_state::system_clock_discipliner::{
    ClockCorrection, SystemClockDiscipliner,
};
use crate::daemon::clock_sync_algorithm::SyncParameters;
use crate::daemon::io::ClockDisruptionEvent;
use crate::daemon::io::tsc::ReadTscImpl;
use crate::daemon::io::vmclock::{State as VMClockState, VMClockParams};
use crate::daemon::logging::synchronization::SNAPSHOT_PERIOD_SECS as SYNC_SNAPSHOT_PERIOD_SECS;
use crate::daemon::logging::synchronization::{
    log_clock_status_change, log_sync_snapshot, log_system_clock_step, log_system_clock_summary,
};
use crate::daemon::time::clocks::{ClockBound, MonotonicCoarse};
use crate::daemon::time::tsc::Skew;
use crate::daemon::time::{Clock, Duration};
use crate::shm::{
    CLOCKBOUND_SHM_DEFAULT_PATH_V0, CLOCKBOUND_SHM_DEFAULT_PATH_V1, ClockErrorBoundLayoutVersion,
    ClockStatus, ShmWriter,
};

const FREE_RUNNING_GRACE_PERIOD: Duration = Duration::from_secs(60);

const CLOCK_ADJUST_SUMMARY_PERIOD_SECS: u64 = 600;

/// The whole `ClockState` component struct.
///
/// This encompasses
/// - the `SystemClockDiscipliner` component which interfaces with the
///   `CLOCK_REALTIME` kernel clock to synchronize it with `ClockBound` estimate
///   of UTC (`ClockBound` clock), and
/// - `Shm0` which manages writing
///   the `ClockErrorBound` to SHM segment for `ClockBoundV2` clients to read
/// - `Shm1` which manages writing
///   the `ClockErrorBound` to SHM segment for `ClockBoundV3` clients to read
pub(crate) struct ClockState {
    shm0: Shm0,
    shm1: Shm1,
    system_clock_discipliner: SystemClockDiscipliner,
    sync_parameters: Option<SyncParameters>,
    clock_status: ClockStatus,
    clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
    clock_params_receiver: Receiver<SyncParameters>,
    cancellation_token: CancellationToken,
    vmclock_state: Option<Arc<Mutex<VMClockState>>>,
    adjust_data: ClockAdjustData,
}

impl ClockState {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        shm0: Shm0,
        shm1: Shm1,
        system_clock_discipliner: SystemClockDiscipliner,
        clock_params_receiver: Receiver<SyncParameters>,
        clock_disruption_receiver: watch::Receiver<ClockDisruptionEvent>,
        cancellation_token: CancellationToken,
        clock_status: ClockStatus,
        vmclock_state: Option<Arc<Mutex<VMClockState>>>,
    ) -> Self {
        Self {
            shm0,
            shm1,
            system_clock_discipliner,
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
        max_dispersion: Skew,
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
        // creation of these writers to the Shm0::new()/Shm1::new() methods.
        //
        let shm_writer_0 = ShmWriter::new(
            Path::new(CLOCKBOUND_SHM_DEFAULT_PATH_V0),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .unwrap();

        let shm_writer_1 = ShmWriter::new(
            Path::new(CLOCKBOUND_SHM_DEFAULT_PATH_V1),
            ClockErrorBoundLayoutVersion::V3,
            ClockErrorBoundLayoutVersion::V3,
        )
        .unwrap();

        // The same oscillator bound the synchronization algorithm uses; always representable
        // (validated positive and far below 1e9 ppb).
        let max_drift_ppb = max_dispersion
            .to_ppb()
            .expect("max_dispersion is a valid ppb value");

        let shm0 = Shm0::builder()
            .shm_writer(Box::new(shm_writer_0))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        let shm1 = Shm1::builder()
            .shm_writer(Box::new(shm_writer_1))
            .clock_disruption_support_enabled(clock_disruption_support_enabled)
            .max_drift_ppb(max_drift_ppb)
            .disruption_marker(disruption_marker)
            .build();
        #[cfg(not(feature = "test-side-by-side"))]
        let system_clock_discipliner = SystemClockDiscipliner::with_kernel_clock_adjuster();
        #[cfg(feature = "test-side-by-side")]
        let system_clock_discipliner = SystemClockDiscipliner::with_noop_clock_adjuster();

        Self::new(
            shm0,
            shm1,
            system_clock_discipliner,
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
        let mut snapshot_interval =
            tokio::time::interval(tokio::time::Duration::from_secs(SYNC_SNAPSHOT_PERIOD_SECS));
        // delay first snapshot log by a few seconds to let startup/source-selection converge.
        snapshot_interval.reset_after(tokio::time::Duration::from_secs(8));
        let mut clock_adjust_summary_interval = tokio::time::interval(
            tokio::time::Duration::from_secs(CLOCK_ADJUST_SUMMARY_PERIOD_SECS),
        );
        // delay first summary log by full period.
        clock_adjust_summary_interval.reset();
        debug!("Starting run for ClockState.");
        // FIXME: This clears the SHM segment quite early, before we have
        // even received `ClockParameters` and started trying to adjust the clock -
        // it is overly cautious. We could hold off even longer before clearing things,
        // in case a previous `ClockBound` has written reliable SHM data.
        // We could consider to wait til we are about to step the clock, and set the
        // clock status to unknown right before then.
        self.shm0.initialize();
        loop {
            tokio::select! {
                biased; // biased to ensure disruption is handled first when this happens
                Ok(()) = self.clock_disruption_receiver.changed() => {
                    self.handle_disruption();
                }
                source_params = self.clock_params_receiver.recv() => {
                    // unwrap ok. If None, it means `Daemon` stopped running..
                    self.handle_sync_parameters(source_params.unwrap());
                },
                _ = snapshot_interval.tick() => {
                    log_sync_snapshot(self.sync_parameters.as_ref(), self.clock_status);
                },
                _ = clock_adjust_summary_interval.tick() => {
                    self.flush_clock_adjust_summary();
                },
                () = self.cancellation_token.cancelled() => {
                    // nothing fancy for now. just exit
                    // TODO: we may want to clean-up SHM here.
                    debug!("Received shutdown signal. Exiting.");
                    break;
                },
            }
        }
        debug!("ClockState runner exiting.");
    }

    /// Determines the `ClockStatus` to write to the SHM for `ClockErrorBoundV2`, which depends on the system
    /// clock underneath the hood.
    fn determine_clock_error_bound_v2_status(&self, parameters: &ClockParameters) -> ClockStatus {
        let clock_status = self.system_clock_discipliner.get_clock_realtime_status();
        let time_since_parameters_updated = MonotonicCoarse.get_time() - parameters.as_of_monotonic;
        Self::check_clock_status_free_running(clock_status, time_since_parameters_updated)
    }

    /// Ages a `ClockStatus` into `ClockStatus::FreeRunning` once synchronized clock parameters
    /// have been stale past [`FREE_RUNNING_GRACE_PERIOD`].
    ///
    /// Only a `Synchronized` status is subject to aging; any other status is returned unchanged.
    fn check_clock_status_free_running(
        status: ClockStatus,
        time_since_parameters_updated: Duration,
    ) -> ClockStatus {
        if status == ClockStatus::Synchronized
            && time_since_parameters_updated >= FREE_RUNNING_GRACE_PERIOD
        {
            return ClockStatus::FreeRunning;
        }
        status
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
            info!("Clock status changed from {previous:?} to {new:?}.");
            log_clock_status_change(previous, new);
        }
    }

    /// Handles `SyncParameters` passed out from the `ClockSyncAlgorithm` selector.
    ///
    /// Stores the parameters and immediately adjusts the clock and writes both SHM segments.
    ///
    /// # Panics
    /// If the `ClockAdjuster` fails, e.g. an invalid value was supplied to `ntp_adjtime`, or
    /// insufficient permissions to adjust the clock.
    pub fn handle_sync_parameters(&mut self, sync_parameters: SyncParameters) {
        self.sync_parameters = Some(sync_parameters);

        // If a VMClock was expected but has failed, the clock status is untrustworthy and both SHM
        // segments must report `ClockStatus::Unknown`.
        let vmclock_failed = self.is_vmclock_in_failed_state();

        if let Some(sp) = &self.sync_parameters {
            let parameters = &sp.clock_parameters;
            match self
                .system_clock_discipliner
                .handle_clock_parameters(parameters)
            {
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

            self.shm1
                .handle_clock_parameters(parameters, clock_status_v3);

            // Handle SHM0 (`ClockErrorboundV2`) after SHM1.
            let mut clock_status_v2 = self.determine_clock_error_bound_v2_status(parameters);
            if vmclock_failed {
                clock_status_v2 = ClockStatus::Unknown;
            }
            self.shm0
                .handle_clock_parameters(parameters, clock_status_v2);
        }
    }

    /// Emit a `system_clock_summary` for the smooth phase corrections accumulated
    /// over the current window and reset the window.
    fn flush_clock_adjust_summary(&mut self) {
        log_system_clock_summary(CLOCK_ADJUST_SUMMARY_PERIOD_SECS, &self.adjust_data);
        self.adjust_data = ClockAdjustData::default();
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
            system_clock_discipliner,
            shm0,
            shm1,
            clock_params_receiver,
            clock_disruption_receiver,
            sync_parameters,
            clock_status,
            cancellation_token: _,
            vmclock_state: _,
            adjust_data: _,
        } = self;

        let val = clock_disruption_receiver.borrow_and_update().clone();
        if let Some(disruption_marker) = val.disruption_marker {
            // Update the clock status on the shared memory segments.
            if let Some(sp) = sync_parameters {
                shm1.handle_disruption(&sp.clock_parameters, disruption_marker);
                shm0.handle_disruption(&sp.clock_parameters, disruption_marker);
            }

            *sync_parameters = None;
            clock_params_receiver.handle_disruption();
            system_clock_discipliner.handle_disruption(disruption_marker);

            // Log clock status transition to Disrupted and update shared state
            Self::set_clock_status(clock_status, ClockStatus::Disrupted);

            tracing::debug!("Handled clock disruption event.");
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::{
        daemon::{
            async_ring_buffer,
            clock_state::shm_writer::test_support::MockShmWriter,
            clock_state::system_clock_discipliner::MockNtpAdjTimeExt,
            clock_sync_algorithm::SourceInfo,
            time::clocks::RealTime,
            time::{
                Duration, Instant, TscCount,
                tsc::{Frequency, Period},
            },
        },
        shm::{ClockErrorBound, ClockStatus},
    };

    use super::*;

    /// Builds a `Shm0` over the supplied mock writer, with inert field values. Tests assert on the
    /// `ClockStatus` written at the `ShmWrite` seam, so the field values here are not significant.
    fn mock_shm0(mock: MockShmWriter) -> Shm0 {
        Shm0::builder()
            .shm_writer(Box::new(mock))
            .clock_disruption_support_enabled(false)
            .max_drift_ppb(0)
            .disruption_marker(0)
            .build()
    }

    /// Builds a `Shm1` over the supplied mock writer, with inert field values.
    fn mock_shm1(mock: MockShmWriter) -> Shm1 {
        Shm1::builder()
            .shm_writer(Box::new(mock))
            .clock_disruption_support_enabled(false)
            .max_drift_ppb(0)
            .disruption_marker(0)
            .build()
    }

    /// This function used to have deterministic values. However, removing
    /// the high level trait on the system clock discipline struct and
    /// relying on the NtpAdjTime struct for IO revealed that the struct is
    /// very hard to test deterministically. Also the NtpAdjtime trait interface needs to be modified
    fn get_sample_clock_parameters() -> ClockParameters {
        // I do not like having system IO running non-deterministically here.
        // But tests rely on TSC and time being close to now for the calls to ntp_adjtime to be sane.
        // TODO: fix handle_sync_parameters_with_parameters
        let tsc_count =
            TscCount::new(crate::daemon::io::tsc::read_timestamp_counter_begin() as i64);
        let realtime_now = RealTime.get_time();
        ClockParameters {
            tsc_count,
            time: realtime_now,
            clock_error_bound: Duration::new(0),
            period: Period::from_frequency(Frequency::from_hz(1_000_000_000.0)),
            period_max_error: Period::from_seconds(0.0),
            as_of_monotonic: realtime_now,
        }
    }

    #[tokio::test]
    async fn handle_disruption() {
        let disruption_marker = 123;
        let cancellation_token = CancellationToken::new();
        // The adjuster's `handle_disruption` is presently a no-op. No expectations necessary
        let system_clock_discipliner =
            SystemClockDiscipliner::new(Box::new(MockNtpAdjTimeExt::new()));
        let clock_parameters = ClockParameters {
            tsc_count: TscCount::new(1000),
            time: Instant::from_nanos(1000),
            clock_error_bound: Duration::from_nanos(1000),
            period: Period::from_seconds(1e-9),
            period_max_error: Period::from_seconds(1e-11),
            as_of_monotonic: Instant::from_nanos(1000),
        };

        let mut mock_shm_writer_1 = MockShmWriter::new();
        mock_shm_writer_1
            .expect_write()
            .once()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.clock_status() == ClockStatus::Disrupted
                    && ceb.disruption_marker() == disruption_marker
            })
            .return_const(());
        let mut mock_shm_writer_0 = MockShmWriter::new();
        mock_shm_writer_0
            .expect_write()
            .once()
            .withf(move |ceb: &ClockErrorBound| {
                ceb.clock_status() == ClockStatus::Disrupted
                    && ceb.disruption_marker() == disruption_marker
            })
            .return_const(());

        let shm0 = mock_shm0(mock_shm_writer_0);
        let shm1 = mock_shm1(mock_shm_writer_1);
        let (_tx, rx) = async_ring_buffer::create(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            shm0,
            shm1,
            system_clock_discipliner,
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

    /// When a VMClock is present but in the `Failed` state, both SHM segments must be written with
    /// `ClockStatus::Unknown`, regardless of what the clock parameters would otherwise imply.
    #[tokio::test]
    async fn handle_sync_parameters_vmclock_failed_forces_unknown() {
        let mut expected_clock_params = get_sample_clock_parameters();
        expected_clock_params.as_of_monotonic = MonotonicCoarse.get_time();
        let cancellation_token = CancellationToken::new();

        // The real adjuster smooth-slews once; the forced-Unknown override happens in
        // `handle_sync_parameters` after the adjuster reports `Synchronized`.
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction_smooth()
            .once()
            .returning(|_, _| Ok(crate::daemon::time::timex::Timex::retrieve()));
        let system_clock_discipliner = SystemClockDiscipliner::new(Box::new(mock_ntp_adjtime));

        let mut mock_shm_writer_1 = MockShmWriter::new();
        mock_shm_writer_1
            .expect_write()
            .once()
            .withf(|ceb: &ClockErrorBound| ceb.clock_status() == ClockStatus::Unknown)
            .return_const(());
        let mut mock_shm_writer_0 = MockShmWriter::new();
        mock_shm_writer_0
            .expect_write()
            .once()
            .withf(|ceb: &ClockErrorBound| ceb.clock_status() == ClockStatus::Unknown)
            .return_const(());
        let shm0 = mock_shm0(mock_shm_writer_0);
        let shm1 = mock_shm1(mock_shm_writer_1);

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let vmclock_state = Some(Arc::new(Mutex::new(VMClockState::Failed)));
        let mut clock_state = ClockState::new(
            shm0,
            shm1,
            system_clock_discipliner,
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            vmclock_state,
        );
        clock_state.handle_sync_parameters(SyncParameters {
            clock_parameters: expected_clock_params,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
    }

    /// When no VMClock is present (`None`) or one is present and `Running`, `handle_sync_parameters`
    /// behaves nominally (no forced Unknown): a fresh set of clock parameters yields `Synchronized`.
    #[rstest]
    #[case::vmclock_none(None)]
    #[case::vmclock_running(Some(Arc::new(Mutex::new(VMClockState::Running))))]
    #[tokio::test]
    async fn handle_sync_parameters_nominal(
        #[case] vmclock_state: Option<Arc<Mutex<VMClockState>>>,
    ) {
        let mut expected_clock_params = get_sample_clock_parameters();
        expected_clock_params.as_of_monotonic = MonotonicCoarse.get_time();
        let cancellation_token = CancellationToken::new();

        // The real adjuster smooth-slews once on the first parameters and reports `Synchronized`.
        let mut mock_ntp_adjtime = MockNtpAdjTimeExt::new();
        mock_ntp_adjtime
            .expect_apply_phase_correction_smooth()
            .once()
            .returning(|_, _| Ok(crate::daemon::time::timex::Timex::retrieve()));
        let system_clock_discipliner = SystemClockDiscipliner::new(Box::new(mock_ntp_adjtime));

        let mut mock_shm_writer_1 = MockShmWriter::new();
        mock_shm_writer_1
            .expect_write()
            .once()
            .withf(|ceb: &ClockErrorBound| ceb.clock_status() == ClockStatus::Synchronized)
            .return_const(());
        let mut mock_shm_writer_0 = MockShmWriter::new();
        mock_shm_writer_0
            .expect_write()
            .once()
            .withf(|ceb: &ClockErrorBound| ceb.clock_status() == ClockStatus::Synchronized)
            .return_const(());
        let shm0 = mock_shm0(mock_shm_writer_0);
        let shm1 = mock_shm1(mock_shm_writer_1);

        let (_tx, rx) = async_ring_buffer::create(1);
        let (_, clock_disruption_receiver) =
            watch::channel::<ClockDisruptionEvent>(ClockDisruptionEvent::default());
        let mut clock_state = ClockState::new(
            shm0,
            shm1,
            system_clock_discipliner,
            rx,
            clock_disruption_receiver,
            cancellation_token,
            ClockStatus::Unknown,
            vmclock_state,
        );
        clock_state.handle_sync_parameters(SyncParameters {
            clock_parameters: expected_clock_params,
            source_info: SourceInfo::Phc("/dev/ptp0".into()),
            selected_at: Instant::new(0),
            selected_at_clock_error_bound: Duration::new(0),
        });
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
                mock_shm0(MockShmWriter::new()),
                mock_shm1(MockShmWriter::new()),
                SystemClockDiscipliner::new(Box::new(MockNtpAdjTimeExt::new())),
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
    fn check_clock_status_free_running(
        #[case] clock_params_age: Duration,
        #[case] clock_adjust_status: ClockStatus,
        #[case] expected_clock_error_bound_v2_status: ClockStatus,
    ) {
        // Only `Synchronized` ages into `FreeRunning` once past the grace period; every other
        // status (including the `Disrupted` and `Unknown` cases the real adjuster never emits
        // but the pure function must still pass through) is returned unchanged.
        let res =
            ClockState::check_clock_status_free_running(clock_adjust_status, clock_params_age);
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
