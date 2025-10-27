//! VMClock Source

use thiserror::Error;
use tokio::{
    fs, io,
    sync::{mpsc, watch},
    time::{Duration, Interval, MissedTickBehavior, interval},
};
use tracing::{debug, info};

use crate::shm::ShmError;
use crate::vmclock::{shm::VMClockShmBody, shm_reader::VMClockShmReader};

use super::{ClockDisruptionEvent, ControlRequest};

const VMCLOCK_TIMEOUT: Duration = Duration::from_millis(100);

/// Indicates the current status of the VMClock.
#[derive(Debug, PartialEq)]
pub enum ClockDisruptionStatus {
    Normal,
    Disrupted,
}

#[derive(Debug, Error)]
pub enum VMClockConstructionError {
    #[error("IO failure.")]
    Io(#[from] io::Error),
    #[error("Error with shared memory file.")]
    ShmError(#[from] ShmError),
    #[error("File does not exist")]
    FileNonexistent(String),
}

/// Contains the data needed to run the VMClock runner.
///
/// The struct contains the data needed to access the VMClock shared memory file,
/// to determine if a clock disruption event has occurred, and send clock disruption events to
/// channel subscribers.
pub struct VMClock {
    /// Path to the vmclock shared memory file.
    path: String,
    /// Interface used to read the shared memory file.
    reader: VMClockShmReader,
    /// Data from the previously read shared memory file.
    previous_shm_body: VMClockShmBody,
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
    /// Upon creation the VMClock shared memory page existence is verified, a reader is
    /// constructed, and the memory page is read to determine the current state of the clock.
    pub async fn construct(
        vmclock_shm_path: &str,
        ctrl_receiver: mpsc::Receiver<ControlRequest>,
        clock_disruption_sender: watch::Sender<ClockDisruptionEvent>,
    ) -> Result<VMClock, VMClockConstructionError> {
        if !fs::try_exists(vmclock_shm_path).await? {
            return Err(VMClockConstructionError::FileNonexistent(
                vmclock_shm_path.into(),
            ));
        }

        let mut reader = VMClockShmReader::new(vmclock_shm_path)?;
        let vmclock_snapshot = *reader.snapshot()?;
        let mut vmclock_interval = interval(VMCLOCK_TIMEOUT);
        vmclock_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        Ok(VMClock {
            path: vmclock_shm_path.into(),
            reader,
            previous_shm_body: vmclock_snapshot,
            interval: interval(VMCLOCK_TIMEOUT),
            ctrl_receiver,
            clock_disruption_sender,
        })
    }

    /// Reads the VMClock shared memory page and returns the current clock state.
    fn sample(&mut self) -> Result<ClockDisruptionStatus, ShmError> {
        let vmclock_snapshot = self.reader.snapshot()?;

        // The marker increments by an indeterminate amount every clock disruption event.
        if self.previous_shm_body.disruption_marker != vmclock_snapshot.disruption_marker {
            self.previous_shm_body.disruption_marker = vmclock_snapshot.disruption_marker;
            return Ok(ClockDisruptionStatus::Disrupted);
        }
        Ok(ClockDisruptionStatus::Normal)
    }

    /// VMClock runner.
    ///
    /// Reads the VMClock shared memory file and sends clock disruption events to channel
    /// subscribers.
    ///
    /// # Panics
    /// - If the `clock_disruption_sender` is unable to send a clock disruption event.
    pub async fn run(&mut self) {
        // Sampling loop
        info!("Starting VMClock sampling loop.");
        loop {
            tokio::select! {
                _ = self.interval.tick() => {
                    match self.sample() {
                        Ok(s) => {
                            if matches!(s, ClockDisruptionStatus::Disrupted) {
                                self.clock_disruption_sender.send(ClockDisruptionEvent{}).unwrap();
                                debug!(?self, "A clock disruption event occurred and a disruption event was sent.");
                            }
                        },
                        Err(e) => debug!(?e, "Failed to sample the VMClock.")
                    }
                }
                _ = self.ctrl_receiver.recv() => {
                    // Ctrl logic here.
                    // Currently we breakout of the loop if we receive a control event.
                    break;
                }
            }
        }
        info!("VMCLock runner exiting.");
    }
}

impl std::fmt::Debug for VMClock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        f.debug_struct("VMClock")
            .field("path", &self.path)
            .field("previous_shm_body", &self.previous_shm_body)
            .field("interval", &self.interval)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use crate::vmclock::shm::VMClockClockStatus;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use tempfile::NamedTempFile;

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

    #[tokio::test]
    async fn vmclock_construction_success() {
        // Create the shared memory file
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent::default();
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent {});
        let _ = VMClock::construct(vmclock_shm_path, ctrl_receiver, clock_disruption_sender)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn vmclock_construction_failure() {
        let filename = "name/of/file/that/shouldnt_exist";

        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent {});
        let construct_result =
            VMClock::construct(filename, ctrl_receiver, clock_disruption_sender).await;
        assert!(construct_result.is_err());
    }

    #[tokio::test]
    async fn vmclock_sample_success() {
        // Create the shared memory file
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent::default();
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent {});
        let mut vmclock =
            VMClock::construct(vmclock_shm_path, ctrl_receiver, clock_disruption_sender)
                .await
                .unwrap();
        let _ = vmclock.sample().unwrap();
    }

    #[tokio::test]
    async fn vmclock_sample_no_clock_disruption() {
        // Create the shared memory file
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent::default();
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        // Construct the VMClock runner
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent {});
        let mut vmclock =
            VMClock::construct(vmclock_shm_path, ctrl_receiver, clock_disruption_sender)
                .await
                .unwrap();

        // Sample the shared memory page
        let clock_status = vmclock.sample().unwrap();
        assert_eq!(clock_status, ClockDisruptionStatus::Normal);
    }

    #[tokio::test]
    async fn vmclock_sample_clock_disruption() {
        // Create the shared memory file
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent::default();
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        // Construct the vmclock runner
        let (_, ctrl_receiver) = mpsc::channel::<ControlRequest>(1);
        let (clock_disruption_sender, _) = watch::channel(ClockDisruptionEvent {});
        let mut vmclock =
            VMClock::construct(vmclock_shm_path, ctrl_receiver, clock_disruption_sender)
                .await
                .unwrap();

        // Update the shared memory file
        let mut vmclock_content = VMClockContent::default();
        vmclock_content.disruption_marker += 1;
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        // Sample the vmclock
        let clock_status = vmclock.sample().unwrap();
        assert_eq!(clock_status, ClockDisruptionStatus::Normal);
    }
}
