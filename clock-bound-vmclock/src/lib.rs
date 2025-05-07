use std::ffi::CString;
use tracing::debug;

use crate::shm_reader::VMClockShmReader;
use clock_bound_shm::{ClockStatus, ShmError, ShmReader};
use nix::sys::time::TimeSpec;

pub mod shm;
pub mod shm_reader;
pub mod shm_writer;

/// VMClock provides the following capabilities:
///
/// - Error-bounded timestamps obtained from ClockBound daemon.
/// - Clock disruption signaling via the VMClock.
pub struct VMClock {
    clockbound_shm_reader: ShmReader,
    vmclock_shm_path: String,
    vmclock_shm_reader: Option<VMClockShmReader>,
}

impl VMClock {
    /// Open the VMClock shared memory segment and the ClockBound shared memory segment for reading.
    ///
    /// On error, returns an appropriate `Errno`. If the content of the segment
    /// is uninitialized, unparseable, or otherwise malformed, EPROTO will be
    /// returned.
    pub fn new(clockbound_shm_path: &str, vmclock_shm_path: &str) -> Result<VMClock, ShmError> {
        let clockbound_shm_path = CString::new(clockbound_shm_path).expect("CString::new failed");
        let mut clockbound_shm_reader = ShmReader::new(clockbound_shm_path.as_c_str())?;
        let clockbound_snapshot = clockbound_shm_reader.snapshot()?;

        let mut vmclock_shm_reader: Option<VMClockShmReader> = None;
        if clockbound_snapshot.clock_disruption_support_enabled {
            vmclock_shm_reader = Some(VMClockShmReader::new(vmclock_shm_path)?);
        }

        Ok(VMClock {
            clockbound_shm_reader,
            vmclock_shm_path: String::from(vmclock_shm_path),
            vmclock_shm_reader,
        })
    }

    /// The VMClock equivalent of clock_gettime(), but with bound on accuracy.
    ///
    /// Returns a pair of (earliest, latest) timespec between which current time exists. The
    /// interval width is twice the clock error bound (ceb) such that:
    ///   (earliest, latest) = ((now - ceb), (now + ceb))
    /// The function also returns a clock status to assert that the clock is being synchronized, or
    /// free-running, or ...
    pub fn now(&mut self) -> Result<(TimeSpec, TimeSpec, ClockStatus), ShmError> {
        // Read from the ClockBound shared memory segment.
        let clockbound_snapshot = self.clockbound_shm_reader.snapshot()?;

        if self.vmclock_shm_reader.is_none() && clockbound_snapshot.clock_disruption_support_enabled
        {
            self.vmclock_shm_reader = Some(VMClockShmReader::new(self.vmclock_shm_path.as_str())?);
        }

        let (earliest, latest, clock_status) = clockbound_snapshot.now()?;

        if clockbound_snapshot.clock_disruption_support_enabled {
            if let Some(ref mut vmclock_shm_reader) = self.vmclock_shm_reader {
                // Read from the VMClock shared memory segment.
                let vmclock_snapshot = vmclock_shm_reader.snapshot()?;

                // Comparing the disruption marker between the VMClock snapshot and the
                // ClockBound snapshot will tell us if the clock status provided by the
                // ClockBound daemon is trustworthy.
                debug!("clock_status: {:?}, vmclock_snapshot.disruption_marker: {:?}, clockbound_snapshot.disruption_marker: {:?}",
                    clock_status, vmclock_snapshot.disruption_marker,
                    clockbound_snapshot.disruption_marker);

                if vmclock_snapshot.disruption_marker == clockbound_snapshot.disruption_marker {
                    // ClockBound's shared memory segment has the latest clock disruption status from
                    // VMClock and this means the clock status here can be trusted.
                    return Ok((earliest, latest, clock_status));
                } else {
                    // ClockBound has stale clock disruption status and it is not up-to-date with
                    // VMClock.

                    // Override the clock disruption status with ClockStatus::Unknown until
                    // ClockBound daemon is able to pick up the latest clock disruption status
                    // from VMClock.
                    return Ok((earliest, latest, ClockStatus::Unknown));
                }
            }
        }

        debug!("clock_status: {:?}", clock_status);
        Ok((earliest, latest, clock_status))
    }
}

#[cfg(test)]
mod t_lib {
    use super::*;

    use clock_bound_shm::{ClockErrorBound, ShmWrite, ShmWriter};
    use std::path::Path;

    use crate::shm::{VMClockClockStatus, VMClockShmBody};
    use crate::shm_writer::{VMClockShmWrite, VMClockShmWriter};
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    macro_rules! vmclockshmbody {
        () => {
            VMClockShmBody {
                disruption_marker: 10,
                flags: 0_u64,
                _padding: [0x00, 0x00],
                clock_status: VMClockClockStatus::Unknown,
                leap_second_smearing_hint: 0,
                tai_offset_sec: 37_i16,
                leap_indicator: 0,
                counter_period_shift: 0,
                counter_value: 0,
                counter_period_frac_sec: 0,
                counter_period_esterror_rate_frac_sec: 0,
                counter_period_maxerror_rate_frac_sec: 0,
                time_sec: 0,
                time_frac_sec: 0,
                time_esterror_nanosec: 0,
                time_maxerror_nanosec: 0,
            }
        };
    }

    /// Helper function to remove files created during unit tests.
    fn remove_file_or_directory(path: &str) {
        // Busy looping on deleting the previous file, good enough for unit test
        let p = Path::new(&path);
        while p.exists() {
            if p.is_dir() {
                std::fs::remove_dir_all(&path).expect("failed to remove file");
            } else {
                std::fs::remove_file(&path).expect("failed to remove file");
            }
        }
    }

    /// Assert that VMClock can be created successfully and now() function successful when
    /// clock_disruption_support_enabled is true and a valid file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_enabled_success() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create and wipe the ClockBound memory segment.
        let ceb = ClockErrorBound::new(
            TimeSpec::new(1, 2),       // as_of
            TimeSpec::new(3, 4),       // void_after
            123,                       // bound_nsec
            10,                        // disruption_marker
            100,                       // max_drift_ppb
            ClockStatus::Synchronized, // clock_status
            true,                      // clock_disruption_support_enabled
        );

        let mut clockbound_shm_writer =
            ShmWriter::new(Path::new(&clockbound_shm_path)).expect("Failed to create a ShmWriter");
        clockbound_shm_writer.write(&ceb);

        // Create and write the VMClock memory segment.
        let vmclock_shm_body = vmclockshmbody!();
        let mut vmclock_shm_writer = VMClockShmWriter::new(Path::new(&vmclock_shm_path))
            .expect("Failed to create a VMClockShmWriter");
        vmclock_shm_writer.write(&vmclock_shm_body);

        // Create the VMClock, and assert that the creation was successful.
        let vmclock_new_result = VMClock::new(&clockbound_shm_path, &vmclock_shm_path);
        match vmclock_new_result {
            Ok(mut vmclock) => {
                // Assert that now() does not return an error.
                let now_result = vmclock.now();
                assert!(now_result.is_ok());
            }
            Err(_) => {
                assert!(false);
            }
        }
    }

    /// Assert that VMClock will fail to be created when clock_disruption_support_enabled
    /// is true and no file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_enabled_failure() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create and wipe the ClockBound memory segment.
        let ceb = ClockErrorBound::new(
            TimeSpec::new(1, 2),       // as_of
            TimeSpec::new(3, 4),       // void_after
            123,                       // bound_nsec
            10,                        // disruption_marker
            100,                       // max_drift_ppb
            ClockStatus::Synchronized, // clock_status
            true,                      // clock_disruption_support_enabled
        );

        let mut clockbound_shm_writer =
            ShmWriter::new(Path::new(&clockbound_shm_path)).expect("Failed to create a ShmWriter");
        clockbound_shm_writer.write(&ceb);

        // Create the VMClock, and assert that the creation was successful.
        let vmclock_new_result = VMClock::new(&clockbound_shm_path, &vmclock_shm_path);
        assert!(vmclock_new_result.is_err());
    }

    /// Assert that VMClock can be created successfully and now() runs successfully
    /// when clock_disruption_support_enabled is false and no file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_not_enabled() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create and wipe the ClockBound memory segment.
        let ceb = ClockErrorBound::new(
            TimeSpec::new(1, 2),       // as_of
            TimeSpec::new(3, 4),       // void_after
            123,                       // bound_nsec
            10,                        // disruption_marker
            100,                       // max_drift_ppb
            ClockStatus::Synchronized, // clock_status
            false,                     // clock_disruption_support_enabled
        );

        let mut clockbound_shm_writer =
            ShmWriter::new(Path::new(&clockbound_shm_path)).expect("Failed to create a ShmWriter");
        clockbound_shm_writer.write(&ceb);

        // Create the VMClock, and assert that the creation was successful.
        // There should be no error even though there is no file located at vmclock_shm_path.
        let vmclock_new_result = VMClock::new(&clockbound_shm_path, &vmclock_shm_path);
        match vmclock_new_result {
            Ok(mut vmclock) => {
                // Assert that now() does not return an error.
                let now_result = vmclock.now();
                assert!(now_result.is_ok());
            }
            Err(_) => {
                assert!(false);
            }
        }
    }
}
