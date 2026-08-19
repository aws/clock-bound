//! VMClock Source

use std::sync::{Arc, Mutex};

use thiserror::Error;
use tokio::{
    fs, io,
    sync::{mpsc, watch},
    time::{Duration, Interval, MissedTickBehavior, interval},
};
use tracing::{error, info};

use crate::shm::ShmError;
use crate::vmclock::{shm::VMClockShmBody, shm_reader::VMClockShmReader};

use super::{ClockDisruptionEvent, ControlRequest};

const VMCLOCK_TIMEOUT: Duration = Duration::from_millis(100);

/// Interval between "VMClock expected but not found" error logs emitted while the VMClock task is
/// in the `Failed` state.
const VMCLOCK_FAILED_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// Contains the data needed to run the VMClock runner.
///
/// The struct contains the data needed to access the VMClock shared memory file,
/// to determine if a clock disruption event has occurred, and send clock disruption events to
/// channel subscribers.
pub struct VMClock {
    /// Path to the vmclock shared memory file.
    path: String,
    /// The publicly observable state, shared with the `ClockState`.
    shared_state: Arc<Mutex<State>>,
    /// The internal working state, owned by the VMClock task.
    internal_state: InternalState,
    /// The polling interval.
    interval: Interval,
    /// The message channel used to receive control requests.
    ctrl_receiver: mpsc::Receiver<ControlRequest>,
    /// The message channel used to send clock disruption events.
    clock_disruption_sender: watch::Sender<ClockDisruptionEvent>,
}

impl VMClock {
    /// Construct a new `VMClock` instance.
    ///
    /// This performs no IO and is not async. The instance starts in the `Failed` state; call
    /// [`VMClock::initialize`] to read the shared memory file and transition to `Running`.
    pub fn construct(
        vmclock_shm_path: &str,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_sender: watch::Sender<ClockDisruptionEvent>,
    ) -> VMClock {
        VMClock {
            path: vmclock_shm_path.into(),
            shared_state: Arc::new(Mutex::new(State::Failed)),
            internal_state: InternalState::Failed,
            interval: interval(VMCLOCK_TIMEOUT),
            ctrl_receiver,
            clock_disruption_sender,
        }
    }

    /// Initialize the VMClock.
    ///
    /// Verifies the VMClock shared memory page exists, constructs a reader, and takes an initial
    /// snapshot to determine the current state of the clock. On success, transitions both the
    /// internal state and the shared state to `Running`. On failure, leaves both in `Failed` and
    /// returns the [`Error`].
    ///
    /// # Errors
    /// - [`Error::FileNonexistent`] if the shared memory file does not exist.
    /// - [`Error::Io`] if checking for the file's existence fails.
    /// - [`Error::ShmError`] if constructing the reader or taking the initial snapshot fails.
    pub async fn initialize(&mut self) -> Result<(), Error> {
        if !fs::try_exists(&self.path).await? {
            return Err(Error::FileNonexistent(self.path.clone()));
        }

        let mut reader = Reader::new(&self.path)?;
        let previous_shm_body = *reader.snapshot()?;

        self.transition_to_running(Running {
            reader,
            previous_shm_body,
        });

        Ok(())
    }

    /// Returns a clone of the shared [`State`] handle.
    pub fn shared_state(&self) -> Arc<Mutex<State>> {
        self.shared_state.clone()
    }

    /// Returns the last VMClock read's disruption marker, or 0 if the VMClock is not running.
    pub fn last_disruption_marker(&self) -> u64 {
        match &self.internal_state {
            InternalState::Running(running) => running.previous_shm_body.disruption_marker,
            InternalState::Failed => 0,
        }
    }

    /// VMClock runner.
    ///
    /// When `Running`, reads the VMClock shared memory file and sends clock disruption events to
    /// channel subscribers. If a sample ever errors out, the task transitions to `Failed` and
    /// continues running in that state.
    ///
    /// When `Failed`, logs an error every [`VMCLOCK_FAILED_LOG_INTERVAL`] stating the VMClock was
    /// expected but not found. This is terminal: the task never attempts to re-initialize.
    ///
    /// In both states, a `ControlRequest::Shutdown` (or a dropped control sender) exits the loop.
    ///
    /// # Panics
    /// - If the `clock_disruption_sender` is unable to send a clock disruption event.
    pub async fn run(&mut self) {
        info!("Starting VMClock runner.");

        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    self.handle_tick();
                }
                ctrl_req = self.ctrl_receiver.recv() => {
                    match ctrl_req {
                        // this select can happen if `SourceIO` drops the ctrl_sender
                        None => break,
                        Some(ControlRequest::Shutdown) => {
                            info!("Received shutdown signal. Exiting.");
                            break;
                        }
                    }
                }
            }
        }
        info!("VMClock runner exiting.");
    }

    /// Handle a single interval tick.
    ///
    /// When `Running`, samples the VMClock and routes the result through
    /// [`VMClock::handle_sample_result`]. When `Failed`, emits the periodic "expected but not
    /// found" error log (the interval period is 10s in this state, so this fires every 10s).
    fn handle_tick(&mut self) {
        match &mut self.internal_state {
            InternalState::Running(running) => {
                let res = running.sample();
                // Could change state based on result
                self.handle_sample_result(res);
            }
            InternalState::Failed => {
                error!("VMClock expected but not found. ClockStatus UNKNOWN");
            }
        }
    }

    /// Route the outcome of a [`Running::sample`] call.
    ///
    /// On a disruption, sends a [`ClockDisruptionEvent`] to subscribers. On error, the sample is
    /// terminal for the `Running` state: transition to `Failed` and continue running in that
    /// state.
    ///
    /// # Panics
    /// - If the `clock_disruption_sender` is unable to send a clock disruption event.
    fn handle_sample_result(&mut self, res: Result<ClockDisruptionStatus, ShmError>) {
        match res {
            Ok(ClockDisruptionStatus::Disrupted(disruption_marker)) => {
                self.clock_disruption_sender
                    .send(ClockDisruptionEvent {
                        disruption_marker: Some(disruption_marker),
                    })
                    .unwrap();
                info!(
                    disruption_marker,
                    "A clock disruption event occurred and a disruption event was sent."
                );
            }
            Ok(ClockDisruptionStatus::Normal) => {}
            Err(e) => {
                error!(
                    ?e,
                    "Failed to sample the VMClock. Transitioning to Failed state."
                );
                self.transition_to_failed();
            }
        }
    }

    /// Transition to the `Running` state.
    ///
    /// Keeps the internal state, the shared state, and the polling interval in sync: the interval
    /// is reset to [`VMCLOCK_TIMEOUT`] so the task samples the shared memory file.
    fn transition_to_running(&mut self, running: Running) {
        self.internal_state = InternalState::Running(Box::new(running));
        self.set_shared_state(State::Running);
        self.interval = interval(VMCLOCK_TIMEOUT);
    }

    /// Transition to the `Failed` state.
    ///
    /// Keeps the internal state, the shared state, and the polling interval in sync: the interval
    /// is reset to [`VMCLOCK_FAILED_LOG_INTERVAL`] (with [`MissedTickBehavior::Delay`] so a
    /// stalled task does not burst-log on catch-up) so the task emits the periodic error log.
    fn transition_to_failed(&mut self) {
        self.internal_state = InternalState::Failed;
        self.set_shared_state(State::Failed);
        let mut failed_interval = interval(VMCLOCK_FAILED_LOG_INTERVAL);
        failed_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        self.interval = failed_interval;
    }

    /// Update the shared state handle.
    fn set_shared_state(&self, state: State) {
        *self.shared_state.lock().unwrap() = state;
    }
}

impl std::fmt::Debug for VMClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let state = self.shared_state.lock().map(|s| *s).ok();
        f.debug_struct("VMClock")
            .field("path", &self.path)
            .field("shared_state", &state)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

/// This is a wrapper for the [`VMClockShmReader`] struct used to bypass its !Send rules.
///
/// [`VMClockShmReader`] has explicit rules against the implementation of `Send`. These were
/// implemented, in part, to protect those using Clockbound as a library, where the underlying
/// pointers in `VMClockShmReader` could result in unexpected behavior due to their unsafe nature.
/// Unlike the `VMClockShmReader` `Reader` is not apart of the external facing library and in the
/// context of the ClockBound daemon its use is constrained such that it can be utilized safely.
/// Within the Clockbound context that means the Reader cannot be copied, the underlying pointers
/// can not be accessed directly and only one task has access to the struct.
struct Reader(VMClockShmReader);

impl Reader {
    fn new(path: &str) -> Result<Self, ShmError> {
        Ok(Reader(VMClockShmReader::new(path)?))
    }

    fn snapshot(&mut self) -> Result<&VMClockShmBody, ShmError> {
        self.0.snapshot()
    }
}

unsafe impl Send for Reader {}

/// Indicates the current status of the VMClock.
#[derive(Debug, PartialEq)]
pub enum ClockDisruptionStatus {
    Normal,
    Disrupted(u64),
}

/// Errors that can occur while initializing the VMClock.
///
/// This is referenced outside this module as `vmclock::Error`.
#[derive(Debug, Error)]
pub enum Error {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Error with shared memory file.")]
    ShmError(#[from] ShmError),
    #[error("File does not exist")]
    FileNonexistent(String),
}

/// The publicly observable state of the VMClock, shared with the `ClockState` component.
///
/// This is a lightweight status flag wrapped in an `Arc<Mutex<..>>` and shared with the
/// `ClockState`. It does not hold any of the working data (the reader / snapshot), which lives in
/// [`InternalState`] on the VMClock task itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// The VMClock could not be initialized, or has errored out while running. In this state the
    /// `ClockState` writes `ClockStatus::Unknown`.
    Failed,
    /// The VMClock is initialized and sampling the shared memory file.
    Running,
}

/// The internal, private state of the VMClock task.
///
/// Unlike [`State`], this holds the actual working data needed to sample the shared memory file.
/// It is never exposed to the `ClockState`.
enum InternalState {
    /// The VMClock is not (or no longer) running. Terminal: once the task enters this state it
    /// does not attempt to re-initialize.
    Failed,
    /// The VMClock is initialized and sampling. Holds the working data needed to sample the
    /// shared memory file.
    Running(Box<Running>),
}

/// The working data owned by the VMClock task while it is sampling the shared memory file.
struct Running {
    reader: Reader,
    previous_shm_body: VMClockShmBody,
}

impl Running {
    /// Reads the VMClock shared memory page and returns the current clock state.
    ///
    /// Updates the stored snapshot's disruption marker when it changes. This is a pure read: it
    /// performs no logging and sends no events. Callers route the result through
    /// [`VMClock::handle_sample_result`].
    fn sample(&mut self) -> Result<ClockDisruptionStatus, ShmError> {
        let vmclock_snapshot = self.reader.snapshot()?;

        // The marker increments by an indeterminate amount every clock disruption event.
        if self.previous_shm_body.disruption_marker != vmclock_snapshot.disruption_marker {
            self.previous_shm_body.disruption_marker = vmclock_snapshot.disruption_marker;
            return Ok(ClockDisruptionStatus::Disrupted(
                vmclock_snapshot.disruption_marker,
            ));
        }
        Ok(ClockDisruptionStatus::Normal)
    }
}

/// Parameters shared with the `ClockState` component.
///
/// Bundles the shared [`State`] handle (kept in sync with the VMClock task) and the disruption
/// marker read at initialization time.
#[derive(Debug, Clone)]
pub struct VMClockParams {
    /// Shared VMClock state. Kept in sync with the VMClock task's internal state.
    pub shared_state: Arc<Mutex<State>>,
    /// The disruption marker read when the VMClock was initialized (0 if it never initialized).
    pub disruption_marker: u64,
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::vmclock::shm::VMClockClockStatus;
    use std::fs::{File, OpenOptions};
    use std::io::{Seek, Write};
    use tempfile::NamedTempFile;
    use tokio::time::timeout;

    /// Test struct used to hold the expected fields in the VMClock shared memory segment.
    #[repr(C)]
    #[derive(Debug, Copy, Clone, PartialEq)]
    struct VMClockContent {
        magic: u32,
        size: u32,
        version: u16,
        counter_id: u8,
        time_type: u8,
        seq_count: u32,
        disruption_marker: u64,
        flags: u64,
        _padding: [u8; 2],
        clock_status: VMClockClockStatus,
        leap_second_smearing_hint: u8,
        tai_offset_sec: i16,
        leap_indicator: u8,
        counter_period_shift: u8,
        counter_value: u64,
        counter_period_frac_sec: u64,
        counter_period_esterror_rate_frac_sec: u64,
        counter_period_maxerror_rate_frac_sec: u64,
        time_sec: u64,
        time_frac_sec: u64,
        time_esterror_nanosec: u64,
        time_maxerror_nanosec: u64,
    }

    impl Default for VMClockContent {
        fn default() -> Self {
            VMClockContent {
                magic: 0x4B4C4356,
                size: 104_u32,
                version: 1_u16,
                counter_id: 1_u8,
                time_type: 0_u8,
                seq_count: 10_u32,
                disruption_marker: 888888_u64,
                flags: 0_u64,
                _padding: [0x00, 0x00],
                clock_status: VMClockClockStatus::Synchronized,
                leap_second_smearing_hint: 0_u8,
                tai_offset_sec: 0_i16,
                leap_indicator: 0_u8,
                counter_period_shift: 0_u8,
                counter_value: 123456_u64,
                counter_period_frac_sec: 0_u64,
                counter_period_esterror_rate_frac_sec: 0_u64,
                counter_period_maxerror_rate_frac_sec: 0_u64,
                time_sec: 0_u64,
                time_frac_sec: 0_u64,
                time_esterror_nanosec: 0_u64,
                time_maxerror_nanosec: 0_u64,
            }
        }
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

    /// Helper that writes valid VMClock content into a temp file and returns the file handle and
    /// path. The `NamedTempFile` is kept alive by the returned handle so the path stays valid.
    fn write_valid_vmclock() -> (NamedTempFile, File, String) {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_path = vmclock_shm_tempfile
            .path()
            .to_str()
            .expect("path is valid utf-8")
            .to_owned();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(&vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent::default();
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);
        (vmclock_shm_tempfile, vmclock_shm_file, vmclock_shm_path)
    }

    fn channels() -> (
        mpsc::Receiver<ControlRequest>,
        watch::Sender<ClockDisruptionEvent>,
    ) {
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent::default());
        (ctrl_receiver, clock_disruption_sender)
    }

    #[tokio::test]
    async fn construct_does_no_io_and_starts_failed() {
        // A path that does not exist: construct must still succeed since it does no IO.
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let vmclock = VMClock::construct(
            "name/of/file/that/shouldnt_exist",
            ctrl_receiver,
            clock_disruption_sender,
        );

        assert!(matches!(vmclock.internal_state, InternalState::Failed));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Failed);
        assert_eq!(vmclock.last_disruption_marker(), 0);
    }

    #[tokio::test]
    async fn initialize_success_transitions_to_running() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);

        vmclock.initialize().await.unwrap();

        assert!(matches!(
            vmclock.internal_state,
            InternalState::Running { .. }
        ));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Running);
        // The default content has a disruption marker of 888888.
        assert_eq!(vmclock.last_disruption_marker(), 888888);
    }

    #[tokio::test]
    async fn initialize_failure_stays_failed() {
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(
            "name/of/file/that/shouldnt_exist",
            ctrl_receiver,
            clock_disruption_sender,
        );

        let result = vmclock.initialize().await;

        assert!(matches!(result, Err(Error::FileNonexistent(_))));
        assert!(matches!(vmclock.internal_state, InternalState::Failed));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Failed);
    }

    #[tokio::test]
    async fn shared_state_getter_reflects_transitions() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);

        let shared = vmclock.shared_state();
        assert_eq!(*shared.lock().unwrap(), State::Failed);

        vmclock.initialize().await.unwrap();
        // The previously obtained clone observes the transition.
        assert_eq!(*shared.lock().unwrap(), State::Running);
    }

    #[tokio::test]
    async fn sample_no_clock_disruption() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();

        let InternalState::Running(running) = &mut vmclock.internal_state else {
            panic!("expected Running state");
        };

        let clock_status = running.sample().unwrap();
        assert_eq!(clock_status, ClockDisruptionStatus::Normal);
    }

    #[tokio::test]
    async fn sample_clock_disruption() {
        let (_tempfile, mut file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();

        // Update the shared memory file to signal a disruption.
        let mut vmclock_content = VMClockContent::default();
        vmclock_content.seq_count += 10;
        vmclock_content.disruption_marker += 1;
        file.rewind().unwrap();
        write_vmclock_content(&mut file, &vmclock_content);

        let InternalState::Running(running) = &mut vmclock.internal_state else {
            panic!("expected Running state");
        };

        let clock_status = running.sample().unwrap();
        assert_eq!(
            clock_status,
            ClockDisruptionStatus::Disrupted(vmclock_content.disruption_marker)
        );
    }

    /// After a successful initialize (Failed -> Running), the polling interval samples at
    /// `VMCLOCK_TIMEOUT`.
    #[tokio::test]
    async fn interval_period_matches_running_state() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);

        vmclock.initialize().await.unwrap();

        assert!(matches!(vmclock.internal_state, InternalState::Running(_)));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Running);
        assert_eq!(vmclock.interval.period(), VMCLOCK_TIMEOUT);
    }

    /// A sample error transitions Running -> Failed and keeps the internal state, shared state,
    /// and interval period in sync (interval switches to `VMCLOCK_FAILED_LOG_INTERVAL`).
    #[tokio::test]
    async fn handle_sample_result_error_transitions_to_failed() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_receiver, clock_disruption_sender) = channels();
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();
        // Sanity: starts Running with the sampling interval.
        assert_eq!(vmclock.interval.period(), VMCLOCK_TIMEOUT);

        vmclock.handle_sample_result(Err(ShmError::SegmentNotInitialized(
            "test-induced error".into(),
        )));

        assert!(matches!(vmclock.internal_state, InternalState::Failed));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Failed);
        assert_eq!(vmclock.interval.period(), VMCLOCK_FAILED_LOG_INTERVAL);
    }

    /// A `Normal` sample result leaves the VMClock Running and does not emit a disruption event.
    #[tokio::test]
    async fn handle_sample_result_normal_stays_running() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel(ClockDisruptionEvent::default());
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();

        vmclock.handle_sample_result(Ok(ClockDisruptionStatus::Normal));

        assert!(matches!(vmclock.internal_state, InternalState::Running(_)));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Running);
        assert_eq!(vmclock.interval.period(), VMCLOCK_TIMEOUT);
        // No event was sent, so the receiver still observes the default marker.
        assert_eq!(clock_disruption_receiver.borrow().disruption_marker, None);
    }

    /// A `Disrupted` sample result sends a `ClockDisruptionEvent` with the marker and stays
    /// Running.
    #[tokio::test]
    async fn handle_sample_result_disrupted_sends_event() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, clock_disruption_receiver) =
            watch::channel(ClockDisruptionEvent::default());
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();

        vmclock.handle_sample_result(Ok(ClockDisruptionStatus::Disrupted(42)));

        // Still Running after a disruption.
        assert!(matches!(vmclock.internal_state, InternalState::Running(_)));
        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Running);
        // The subscriber observes the disruption marker.
        assert_eq!(
            clock_disruption_receiver.borrow().disruption_marker,
            Some(42)
        );
    }

    /// When the VMClock is Running and receives a shutdown, the runner exits cleanly.
    #[tokio::test]
    async fn run_running_honors_shutdown() {
        let (_tempfile, _file, path) = write_valid_vmclock();
        let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent::default());
        let mut vmclock = VMClock::construct(&path, ctrl_receiver, clock_disruption_sender);
        vmclock.initialize().await.unwrap();

        ctrl_sender.send(ControlRequest::Shutdown).await.unwrap();
        // Should return promptly rather than looping forever.
        timeout(Duration::from_secs(1), vmclock.run())
            .await
            .unwrap();
    }

    /// When the VMClock is Failed, the runner still honors shutdown (and would otherwise only emit
    /// the periodic error log).
    #[tokio::test]
    async fn run_failed_honors_shutdown() {
        let (ctrl_sender, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent::default());
        let mut vmclock = VMClock::construct(
            "name/of/file/that/shouldnt_exist",
            ctrl_receiver,
            clock_disruption_sender,
        );
        // No initialize -> stays Failed.
        ctrl_sender.send(ControlRequest::Shutdown).await.unwrap();
        vmclock.run().await;

        assert_eq!(*vmclock.shared_state().lock().unwrap(), State::Failed);
    }
}
