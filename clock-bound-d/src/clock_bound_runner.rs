use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::chrony_client::ChronyClientExt;
use crate::clock_snapshot_poller::{ClockStatusSnapshot, ClockStatusSnapshotPoller};
use crate::clock_state_fsm::{FSMState, ShmClockState};
use crate::clock_state_fsm_no_disruption::ShmClockStateNoDisruption;
use crate::{
    ChronyClockStatus, ClockDisruptionState, FORCE_DISRUPTION_PENDING, FORCE_DISRUPTION_STATE,
};
use clock_bound_shm::common::{clock_gettime_safe, CLOCK_MONOTONIC};
use clock_bound_shm::{ClockErrorBound, ClockStatus, ShmWrite};
use clock_bound_vmclock::shm::VMClockShmBody;
use clock_bound_vmclock::shm_reader::VMClockShmReader;
use nix::sys::time::TimeSpec;
use retry::delay::Fixed;
use retry::retry;
use tracing::error;
use tracing::{debug, info};

/// The chronyd daemon may be restarted from time to time. This may not necessarily implies that
/// the clock error should not be trusted. This constant defines the amount of time the clock can
/// be kept in FREE_RUNNING mode, before moving to UNKNOWN.
const CHRONY_RESTART_GRACE_PERIOD: Duration = Duration::from_secs(5);

/// Number of chronyd reset retries in a row that we should take
/// before waiting for cooldown duration.
const CHRONY_RESET_NUM_RETRIES: usize = 29;

/// Duration to sleep after attempting to reset and burst chronyd's sources.
const CHRONY_RESET_COOLDOWN_DURATION: Duration = Duration::from_secs(10);

/// Central state of the ClockBound daemon.
/// This struct holds all the internal state of the ClockBound daemon.
pub(crate) struct ClockBoundRunner {
    /// State: FSM that tracks the status of the clock written to the SHM segment.
    shm_clock_state: Box<dyn FSMState>,
    /// State: The last calculated clock error bound based on a snapshot of clock sync info.
    bound_nsec: i64,
    /// State: The time at which a clock status snapshot was sampled last.
    as_of: TimeSpec,
    /// State: The count of clock disruption events.
    disruption_marker: u64,
    /// Config: Maximum drift rate of the clock between updates of the synchronization daemon.
    max_drift_ppb: u32,
    /// Config: Flag indicating whether or not clock disruption support is enabled.
    clock_disruption_support_enabled: bool,
}

impl ClockBoundRunner {
    pub fn new(clock_disruption_support_enabled: bool, max_drift_ppb: u32) -> Self {
        // Select a FSM that supports (or doesn't support) clock disruption
        if clock_disruption_support_enabled {
            ClockBoundRunner {
                shm_clock_state: Box::<ShmClockState>::default(),
                bound_nsec: 0,
                as_of: TimeSpec::new(0, 0),
                disruption_marker: 0,
                max_drift_ppb,
                clock_disruption_support_enabled,
            }
        } else {
            ClockBoundRunner {
                shm_clock_state: Box::<ShmClockStateNoDisruption>::default(),
                bound_nsec: 0,
                as_of: TimeSpec::new(0, 0),
                disruption_marker: 0,
                max_drift_ppb,
                clock_disruption_support_enabled,
            }
        }
    }

    /// Write the clock error bound to the shared memory segment
    ///
    /// The internal state and parameters kept on the ShmUpdater allow to call this function on any
    /// external event received.
    fn write_clock_error_bound(&mut self, shm_writer: &mut impl ShmWrite) {
        // Set a future point in time by which a stale value of the error bound accessed by a
        // reader should not be used. For now, set it to 1000 seconds, which maps to how long a
        // linear model of drift is valid. This is fairly arbitrary and needs to be revisited.
        //
        // TODO: calibrate the value passed to void_after
        let void_after = TimeSpec::new(self.as_of.tv_sec() + 1000, 0);

        let ceb = ClockErrorBound::new(
            self.as_of,
            void_after,
            self.bound_nsec,
            self.disruption_marker,
            self.max_drift_ppb,
            self.shm_clock_state.value(),
            self.clock_disruption_support_enabled,
        );

        debug!("Writing ClockErrorBound to shared memory {:?}", ceb);
        shm_writer.write(&ceb);
    }

    /// Handles all ClockDisruptionState sources, transitioning the FSM as needed. Today, these are:
    ///  - User-sent signals (SIGUSR1/2)
    ///  - VMClock Disruption Marker checking
    ///
    /// We defer vmclock snapshot handling of "disruption state", if a "forced disruption" is pending.
    /// That "forced disruption" should be handled on the next call, unless another SIGUSR1/2 comes in.
    fn handle_disruption_sources(
        &mut self,
        shm_writer: &mut impl ShmWrite,
        vm_clock_reader: &mut Option<VMClockShmReader>,
    ) {
        if FORCE_DISRUPTION_PENDING.load(Ordering::SeqCst) {
            info!("FORCE_DISRUPTION_PENDING was set, handling forced disruption");
            self.handle_forced_disruption_state(shm_writer);
            FORCE_DISRUPTION_PENDING.store(false, Ordering::SeqCst);
        } else {
            // Check for clock disruptions if we are running with clock disruption support.
            if let Some(ref mut vm_clock_reader) = vm_clock_reader {
                match vm_clock_reader.snapshot() {
                    Ok(snapshot) => self.handle_vmclock_disruption_marker(snapshot),
                    Err(e) => error!(
                        "Failed to snapshot the VMClock shared memory segment: {:?}",
                        e
                    ),
                }
            }
        }
    }

    /// Handler for forced disruption state scenario.
    ///
    /// We will always apply ClockDisruptionState::Disrupted if we saw there was a disruption pending set, even
    /// if FORCE_DISRUPTION_STATE is not set, in case that FORCE_DISRUPTION_STATE flipped quickly and we could have missed
    /// an actual disruption. That case could happen if SIGUSR1/SIGUSR2 are sent consecutively before FORCE_DISRUPTION_STATE
    /// is checked.
    fn handle_forced_disruption_state(&mut self, shm_writer: &mut impl ShmWrite) {
        info!("Applying ClockDisruptionState::Disrupted and waiting for disruption state to be set to false");
        self.shm_clock_state = self
            .shm_clock_state
            .apply_disruption(ClockDisruptionState::Disrupted);
        // We have to write this state to the SHM in this case - otherwise, the client cannot see that it is disrupted
        // (vmclock and ClockBound SHM segment might not differ in disruption marker)
        self.write_clock_error_bound(shm_writer);
        while FORCE_DISRUPTION_STATE.load(Ordering::SeqCst) {
            info!("FORCE_DISRUPTION_STATE is still true, waiting for it to be set to false, ClockBound will do no other work at this time");
            std::thread::sleep(Duration::from_secs(1));
        }
        info!("FORCE_DISRUPTION_STATE is now false, continuing execution of ClockBound");
    }

    /// Handles checking VMClock disruption marker
    ///
    /// Today, this is only used for detecting whether VMClock has "disrupted" the clock.
    fn handle_vmclock_disruption_marker(&mut self, current_snapshot: &VMClockShmBody) {
        // We've seen a change in our disruption marker, so we should apply "Disrupted" and update our disruption marker.
        if self.disruption_marker != current_snapshot.disruption_marker {
            debug!(
                "Disruption marker changed from {:?} to {:?}",
                self.disruption_marker, current_snapshot.disruption_marker
            );
            debug!("Current VMClock snapshot: {:?}", current_snapshot);
            self.shm_clock_state = self
                .shm_clock_state
                .apply_disruption(ClockDisruptionState::Disrupted);
            self.disruption_marker = current_snapshot.disruption_marker;
        } else {
            // If the disruption marker is consistent across reads, then at this point we can assume our ClockDisruptionState
            // is reliable.
            self.shm_clock_state = self
                .shm_clock_state
                .apply_disruption(ClockDisruptionState::Reliable);
        }
    }

    /// Processes a ClockStatusSnapshot.
    ///
    /// This snapshot is used to update the error bound value written to ClockBound SHM,
    /// and the clock state FSM.
    fn apply_clock_status_snapshot(&mut self, snapshot: &ClockStatusSnapshot) {
        debug!("Current ClockStatusSnapshot: {:?}", snapshot);
        self.shm_clock_state = self
            .shm_clock_state
            .apply_chrony(snapshot.chrony_clock_status);
        // Only update the clock error bound value if chrony is synchronized. This helps ensure
        // that the default value of root delay and root dispersion (both set to 1 second) do not
        // distort the linear growth of the clock error bound when chronyd restarts.
        if snapshot.chrony_clock_status == ChronyClockStatus::Synchronized {
            self.bound_nsec = snapshot.error_bound_nsec;
            self.as_of = snapshot.as_of;
        }
    }

    /// Handles the case where a clock status snapshot was not retrieved successfully
    /// (Chronyd may be non-responsive, or VMClock polling failed.)
    ///
    /// If beyond our grace period, clock status "Unknown" should be applied.
    fn handle_missing_clock_status_snapshot(&mut self, as_of: TimeSpec) {
        if (as_of - self.as_of) < TimeSpec::from_duration(CHRONY_RESTART_GRACE_PERIOD) {
            debug!("Current timestamp is within grace period for Chronyd restarts, applying ChronyClockStatus::FreeRunning");
            self.shm_clock_state = self
                .shm_clock_state
                .apply_chrony(ChronyClockStatus::FreeRunning);
        } else {
            debug!("Current timestamp is not within grace period for Chronyd restarts, applying ChronyClockStatus::Unknown");
            self.shm_clock_state = self
                .shm_clock_state
                .apply_chrony(ChronyClockStatus::Unknown);
        }
    }

    /// Processes the current FSM state, performing any work or transitions needed.
    ///
    /// Currently, we only check if we're "Disrupted", and try continually to reset Chronyd
    /// if we are, followed by applying ClockDisruptionState::Unknown.
    fn process_current_fsm_state(&mut self, chrony_client: &impl ChronyClientExt) {
        match self.shm_clock_state.value() {
            ClockStatus::Disrupted => {
                // This will continue to retry FOREVER until it succeeds. This is what we intend, since if our clock was "disrupted",
                // resetting chronyd is a MUST, else chronyd will report stale and possibly dishonest tracking data.
                let _ = retry(Fixed::from(CHRONY_RESET_COOLDOWN_DURATION), || {
                    chrony_client.reset_chronyd_with_retries(CHRONY_RESET_NUM_RETRIES)
                });
                self.shm_clock_state = self
                    .shm_clock_state
                    .apply_disruption(ClockDisruptionState::Unknown);
            }
            _ => {
                // Do nothing
            }
        }
    }

    /// The "main loop" of the ClockBound daemon.
    /// 1. Handle any ClockDisruptionState sources.
    /// 2. Handle any ClockStatusSnapshot sources.
    /// 3. Handle the state of the ClockState FSM.
    /// 4. Write into the ClockBound SHM segment, which our clients read the clock error bound and current ClockStatus from.
    pub(crate) fn run(
        &mut self,
        vm_clock_reader: &mut Option<VMClockShmReader>,
        shm_writer: &mut impl ShmWrite,
        clock_status_snapshot_poller: impl ClockStatusSnapshotPoller,
        chrony_client: impl ChronyClientExt,
    ) {
        loop {
            self.handle_disruption_sources(shm_writer, vm_clock_reader);

            if self.shm_clock_state.value() == ClockStatus::Disrupted {
                info!("Clock is disrupted");
                self.process_current_fsm_state(&chrony_client);
            }

            // First, make sure we take a MONOTONIC timestamp *before* getting ClockSyncInfoSnapshot data. This will
            // slightly inflate the dispersion component of the clock error bound but better be
            // pessimistic and correct, than greedy and wrong. The actual error added here is expected
            // to be small. For example, let's assume a 50PPM drift rate. Let's also assume it takes 10
            // milliseconds for chronyd to respond. This will inflate the CEB by 500 nanoseconds.
            // Assuming it takes 1 second (the timeout of our requests to chronyd), this would inflate the CEB by 50 microseconds.
            match clock_gettime_safe(CLOCK_MONOTONIC) {
                Ok(as_of) => {
                    match clock_status_snapshot_poller.retrieve_clock_status_snapshot(as_of) {
                        Ok(snapshot) => self.apply_clock_status_snapshot(&snapshot),
                        Err(e) => {
                            error!(
                                error = ?e,
                                "Failed to get clock status snapshot"
                            );
                            self.handle_missing_clock_status_snapshot(as_of);
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to get current monotonic timestamp {:?}", e);
                }
            }

            self.process_current_fsm_state(&chrony_client);
            // Finally, write to Clockbound SHM.
            self.write_clock_error_bound(shm_writer);
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

#[cfg(test)]
mod t_clockbound_state_manager {
    use std::fs::{File, OpenOptions};
    use std::io::Write;

    use rstest::rstest;
    use serial_test::serial;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    use clock_bound_vmclock::shm::VMClockClockStatus;

    use crate::chrony_client::MockChronyClientExt;

    use super::*;

    /// Test struct used to hold the expected fields in the VMClock shared memory segment.
    #[repr(C)]
    #[derive(Debug, Copy, Clone, PartialEq, bon::Builder)]
    struct VMClockContent {
        #[builder(default = 0x4B4C4356)]
        magic: u32,
        #[builder(default = 104_u32)]
        size: u32,
        #[builder(default = 1_u16)]
        version: u16,
        #[builder(default = 1_u8)]
        counter_id: u8,
        #[builder(default)]
        time_type: u8,
        #[builder(default)]
        seq_count: u32,
        #[builder(default)]
        disruption_marker: u64,
        #[builder(default)]
        flags: u64,
        #[builder(default)]
        _padding: [u8; 2],
        #[builder(default = VMClockClockStatus::Synchronized)]
        clock_status: VMClockClockStatus,
        #[builder(default)]
        leap_second_smearing_hint: u8,
        #[builder(default)]
        tai_offset_sec: i16,
        #[builder(default)]
        leap_indicator: u8,
        #[builder(default)]
        counter_period_shift: u8,
        #[builder(default)]
        counter_value: u64,
        #[builder(default)]
        counter_period_frac_sec: u64,
        #[builder(default)]
        counter_period_esterror_rate_frac_sec: u64,
        #[builder(default)]
        counter_period_maxerror_rate_frac_sec: u64,
        #[builder(default)]
        time_sec: u64,
        #[builder(default)]
        time_frac_sec: u64,
        #[builder(default)]
        time_esterror_nanosec: u64,
        #[builder(default)]
        time_maxerror_nanosec: u64,
    }

    impl Into<VMClockShmBody> for VMClockContent {
        fn into(self) -> VMClockShmBody {
            VMClockShmBody {
                disruption_marker: self.disruption_marker,
                flags: self.flags,
                _padding: self._padding,
                clock_status: self.clock_status,
                leap_second_smearing_hint: self.leap_second_smearing_hint,
                tai_offset_sec: self.tai_offset_sec,
                leap_indicator: self.leap_indicator,
                counter_period_shift: self.counter_period_shift,
                counter_value: self.counter_value,
                counter_period_frac_sec: self.counter_period_frac_sec,
                counter_period_esterror_rate_frac_sec: self.counter_period_esterror_rate_frac_sec,
                counter_period_maxerror_rate_frac_sec: self.counter_period_maxerror_rate_frac_sec,
                time_sec: self.time_sec,
                time_frac_sec: self.time_frac_sec,
                time_esterror_nanosec: self.time_esterror_nanosec,
                time_maxerror_nanosec: self.time_maxerror_nanosec,
            }
        }
    }

    mockall::mock! {
        pub ShmWrite {}
        impl ShmWrite for ShmWrite {
            fn write(&mut self, ceb: &ClockErrorBound);
        }
    }

    /// Helper to build a SHM clock state - the default starts off unknown,
    /// then we apply chrony transitions and disruption transitions so we reach an intended end state.
    /// Each `apply_*` result depends on the current state of FSM and the current ChronyClockStatus and ClockDisruptionState
    /// but to simplify things we just start off unknown and apply chrony then disruption states.
    fn build_shm_clock_state(
        chrony_clock_status: ChronyClockStatus,
        clock_disruption_state: ClockDisruptionState,
    ) -> Box<dyn FSMState> {
        Box::<ShmClockState>::default()
            .apply_chrony(chrony_clock_status)
            .apply_disruption(clock_disruption_state)
    }

    fn write_vmclock_content(file: &mut File, vmclock_content: &VMClockContent) {
        // Convert the VMClockShmBody struct into a slice so we can write it all out, fairly magic.
        // Definitely needs the #[repr(C)] layout.
        let slice = unsafe {
            ::core::slice::from_raw_parts(
                (vmclock_content as *const VMClockContent) as *const u8,
                ::core::mem::size_of::<VMClockContent>(),
            )
        };

        file.write_all(slice).expect("Write failed VMClockContent");
        file.sync_all().expect("Sync to disk failed");
    }

    fn write_mock_vmclock_content(
        vmclock_shm_tempfile: &NamedTempFile,
        vmclock_content: &VMClockContent,
    ) {
        let vmclock_shm_temppath = vmclock_shm_tempfile.path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);
    }

    #[rstest]
    #[case::start_synchronized_and_with_same_disruption_marker_should_stay_synchronized(
        VMClockContent::builder().disruption_marker(0).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Synchronized,
        0
    )]
    #[case::start_synchronized_and_with_different_disruption_marker_should_get_disrupted(
        VMClockContent::builder().disruption_marker(1).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Disrupted,
        1
    )]
    #[case::start_unknown_and_with_same_disruption_marker_should_become_synchronized(
        VMClockContent::builder().disruption_marker(0).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Unknown),
        ClockStatus::Unknown,
        ClockStatus::Synchronized,
        0
    )]
    #[case::start_unknown_and_with_different_disruption_marker_should_become_disrupted(
        VMClockContent::builder().disruption_marker(1).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Unknown),
        ClockStatus::Unknown,
        ClockStatus::Disrupted,
        1
    )]
    #[case::start_disrupted_and_with_same_disruption_marker_should_become_unknown(
        VMClockContent::builder().disruption_marker(0).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Disrupted),
        ClockStatus::Disrupted,
        ClockStatus::Unknown,
        0
    )]
    #[case::start_disrupted_and_with_different_disruption_marker_should_stay_disrupted(
        VMClockContent::builder().disruption_marker(1).build().into(),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Disrupted),
        ClockStatus::Disrupted,
        ClockStatus::Disrupted,
        1
    )]
    fn test_handle_vmclock_disruption_marker(
        #[case] vmclock_shm_body: VMClockShmBody,
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] expected_final_clock_status: ClockStatus,
        #[case] expected_disruption_marker: u64,
    ) {
        let mut clockbound_state_manager = ClockBoundRunner::new(
            // Clock disruption enabled.
            true, 0,
        );
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );
        clockbound_state_manager.handle_vmclock_disruption_marker(&vmclock_shm_body);
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
        assert_eq!(
            clockbound_state_manager.disruption_marker,
            expected_disruption_marker
        );
    }

    #[rstest]
    #[case::start_unknown_and_apply_synchronized_snapshot_should_get_us_synchronized(
        build_shm_clock_state(ChronyClockStatus::Unknown, ClockDisruptionState::Reliable),
        ClockStatus::Unknown,
        ClockStatusSnapshot {
            chrony_clock_status: ChronyClockStatus::Synchronized,
            error_bound_nsec: 123,
            as_of: TimeSpec::new(456, 789),
        },
        ClockStatus::Synchronized,
        123,
        TimeSpec::new(456, 789),
    )]
    #[case::start_synchronized_and_apply_freerunning_snapshot_should_get_us_freerunning(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatusSnapshot {
            chrony_clock_status: ChronyClockStatus::FreeRunning,
            error_bound_nsec: 123,
            as_of: TimeSpec::new(456, 789),
        },
        ClockStatus::FreeRunning,
        123,
        TimeSpec::new(456, 789),
    )]
    fn test_apply_clock_status_snapshot(
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] snapshot_to_apply: ClockStatusSnapshot,
        #[case] expected_final_clock_status: ClockStatus,
        #[case] expected_bound_nsec: i64,
        #[case] expected_as_of: TimeSpec,
    ) {
        let mut clockbound_state_manager = ClockBoundRunner::new(true, 0);
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );
        clockbound_state_manager.apply_clock_status_snapshot(&snapshot_to_apply);

        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
        if snapshot_to_apply.chrony_clock_status == ChronyClockStatus::Synchronized {
            assert_eq!(clockbound_state_manager.bound_nsec, expected_bound_nsec);
            assert_eq!(clockbound_state_manager.as_of, expected_as_of);
        } else {
            assert_eq!(clockbound_state_manager.bound_nsec, 0);
            assert_eq!(clockbound_state_manager.as_of, TimeSpec::new(0, 0));
        }
    }

    #[rstest]
    #[case::within_grace_period_so_freerunning_is_applied(
        TimeSpec::new(0, 0),
        TimeSpec::new(0, 0),
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::FreeRunning
    )]
    #[case::beyond_grace_period_so_unknown_is_applied(
        TimeSpec::new(0, 0),
        TimeSpec::new(5, 0), // current time as_of is 5 seconds after the initial as_of
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Unknown
    )]
    fn test_handle_missing_clock_status_snapshot(
        #[case] initial_as_of: TimeSpec,
        #[case] current_time_as_of: TimeSpec,
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] expected_final_clock_status: ClockStatus,
    ) {
        let mut clockbound_state_manager = ClockBoundRunner::new(true, 0);
        clockbound_state_manager.as_of = initial_as_of;
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );
        clockbound_state_manager.handle_missing_clock_status_snapshot(current_time_as_of);
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
    }

    #[rstest]
    #[case::clock_is_synchronized_should_be_noop(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Synchronized
    )]
    #[case::clock_is_unknown_should_be_noop(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Unknown),
        ClockStatus::Unknown,
        ClockStatus::Unknown
    )]
    #[case::clock_is_disrupted_should_reset_chronyd_and_apply_unknown(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Disrupted),
        ClockStatus::Disrupted,
        ClockStatus::Unknown
    )]
    fn test_process_current_fsm_state(
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] expected_final_clock_status: ClockStatus,
    ) {
        let mut clockbound_state_manager = ClockBoundRunner::new(true, 0);
        let mut mock_chrony_client = MockChronyClientExt::new();
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );
        if clockbound_state_manager.shm_clock_state.value() != ClockStatus::Disrupted {
            mock_chrony_client
                .expect_reset_chronyd_with_retries()
                .never();
        } else {
            mock_chrony_client
                .expect_reset_chronyd_with_retries()
                .once()
                .with(mockall::predicate::eq(CHRONY_RESET_NUM_RETRIES))
                .return_once(|_| Ok(()));
        }
        clockbound_state_manager.process_current_fsm_state(&mock_chrony_client);
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
    }

    #[rstest]
    #[case::no_forced_disruption_and_disruption_marker_is_consistent(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Synchronized,
        false,
        0,
        0
    )]
    #[case::force_disruption_pending_true_with_consistent_disruption_marker(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Disrupted,
        true,
        0,
        0
    )]
    #[case::force_disruption_pending_true_with_changed_disruption_marker_should_not_handle_disruption_marker(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Disrupted,
        true,
        1,
        0,
    )]
    #[serial]
    fn test_handle_disruption_sources(
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] expected_final_clock_status: ClockStatus,
        #[case] initial_forced_disruption_pending: bool,
        #[case] disruption_marker_to_write_to_vmclock: u64,
        #[case] expected_disruption_marker: u64,
    ) {
        let mut mock_shm_writer = MockShmWrite::new();
        if initial_forced_disruption_pending {
            mock_shm_writer.expect_write().once().return_const(());
        } else {
            mock_shm_writer.expect_write().never();
        }
        let mut clockbound_state_manager = ClockBoundRunner::new(true, 0);
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );

        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        // disruption marker is 0, which is same as our default in ClockBoundRunner
        write_mock_vmclock_content(
            &vmclock_shm_tempfile,
            &VMClockContent::builder()
                .disruption_marker(disruption_marker_to_write_to_vmclock)
                .build(),
        );
        let vm_clock_reader =
            VMClockShmReader::new(vmclock_shm_tempfile.path().to_str().unwrap()).unwrap();
        FORCE_DISRUPTION_PENDING.store(initial_forced_disruption_pending, Ordering::SeqCst);
        clockbound_state_manager
            .handle_disruption_sources(&mut mock_shm_writer, &mut Some(vm_clock_reader));
        // Clear the disruption pending signal to avoid polluting other tests
        FORCE_DISRUPTION_PENDING.store(false, Ordering::SeqCst);
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
        assert_eq!(
            clockbound_state_manager.disruption_marker,
            expected_disruption_marker
        );
    }

    #[rstest]
    #[case::clock_is_synchronized_should_become_disrupted(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Reliable),
        ClockStatus::Synchronized,
        ClockStatus::Disrupted
    )]
    #[case::clock_is_unknown_should_become_disrupted(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Unknown),
        ClockStatus::Unknown,
        ClockStatus::Disrupted
    )]
    #[case::clock_is_disrupted_should_stay_disrupted(
        build_shm_clock_state(ChronyClockStatus::Synchronized, ClockDisruptionState::Disrupted),
        ClockStatus::Disrupted,
        ClockStatus::Disrupted
    )]
    fn test_handle_forced_disruption_state(
        #[case] initial_clock_fsm: Box<dyn FSMState>,
        #[case] expected_initial_clock_status: ClockStatus,
        #[case] expected_final_clock_status: ClockStatus,
    ) {
        let mut mock_shm_writer = MockShmWrite::new();
        mock_shm_writer.expect_write().once().return_const(());
        let mut clockbound_state_manager = ClockBoundRunner::new(true, 0);
        clockbound_state_manager.shm_clock_state = initial_clock_fsm;
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_initial_clock_status
        );
        clockbound_state_manager.handle_forced_disruption_state(&mut mock_shm_writer);
        assert_eq!(
            clockbound_state_manager.shm_clock_state.value(),
            expected_final_clock_status
        );
    }
}
