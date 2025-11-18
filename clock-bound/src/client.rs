//! A client library to communicate with ClockBound daemon. This client library is written in pure Rust.
//!
pub use crate::shm::CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH;
pub use crate::shm::ClockStatus;
use crate::shm::ShmReader;
use crate::shm::{ClockBoundNowResult, ClockBoundSnapshot, ClockErrorBound, ShmError};
pub use crate::vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;
use crate::vmclock::shm_reader::VMClockShmReader;
use errno::Errno;
use std::ffi::CString;
use std::path::Path;

/// The `ClockBoundClient`
///
/// Use it to return current time, the clock error bound and clock status associated with it.
pub struct ClockBoundClient {
    clockbound_shm: ClockBoundSHM,
    vmclock_shm: VMClockSHM,
}

impl ClockBoundClient {
    /// Creates and returns a new `ClockBoundClient`.
    ///
    /// The client accesses two shared memory segments. One written to by the ClockBound daemon.
    /// The second one by the VMClock device (if available).
    ///
    /// Use default paths to the two shared memory segments.
    ///
    /// # Errors
    /// Returns [`ClockBoundError`] if the shared memory segments cannot be open or accessed.
    pub fn new() -> Result<ClockBoundClient, ClockBoundError> {
        Self::new_with_path(CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH)
    }

    /// Creates and returns a new `ClockBoundClient`.
    ///
    /// The client accesses two shared memory segments. One written to by the ClockBound daemon.
    /// The second one by the VMClock device (if available).
    ///
    /// Specify the path to the shared memory segment written to by the VMClock device.
    /// Use the default paths to the ClockBound daemon shared memory segment.
    ///
    /// # Errors
    /// Returns [`ClockBoundError`] if the shared memory segments cannot be open or accessed.
    pub fn new_with_path(clockbound_shm_path: &str) -> Result<ClockBoundClient, ClockBoundError> {
        Self::new_with_paths(clockbound_shm_path, VMCLOCK_SHM_DEFAULT_PATH)
    }

    /// Creates and returns a new `ClockBoundClient`.
    ///
    /// The client accesses two shared memory segments. One written to by the ClockBound daemon.
    /// The second one by the VMClock device (if available).
    ///
    /// Explicitly specifies the paths to the two shared memory segments.
    ///
    /// # Errors
    /// Returns [`ClockBoundError`] if the shared memory segments cannot be open or accessed.
    pub fn new_with_paths(
        clockbound_shm_path: &str,
        vmclock_shm_path: &str,
    ) -> Result<ClockBoundClient, ClockBoundError> {
        // Create the clockbound shared memory accessor
        let mut clockbound_shm = ClockBoundSHM::new(clockbound_shm_path)?;

        // Read the segment to determine whether the daemon has been instructed to provide support
        // for clock disruption. If true, the VMClock will be accessed.
        let cb_snapshot = clockbound_shm.snapshot()?;

        // Create the VMClock shared memory accessor
        let vmclock_shm = VMClockSHM::new(
            vmclock_shm_path,
            cb_snapshot.clock_disruption_support_enabled(),
        )?;

        Ok(ClockBoundClient {
            clockbound_shm,
            vmclock_shm,
        })
    }

    /// Read the current time, but with a bound on accuracy and a status.
    ///
    /// Returns a pair of (earliest, latest) timespec between which current time exists. The
    /// interval width is twice the clock error bound (ceb) such that:
    ///   (earliest, latest) = ((now - ceb), (now + ceb))
    ///
    /// The function also returns a clock status to assert that the clock is being synchronized, or
    /// free-running, or ...
    ///
    /// # Errors
    /// Returns [`ClockBoundError`] if the shared memory segments cannot be open or accessed.
    pub fn now(&mut self) -> Result<ClockBoundNowResult, ClockBoundError> {
        // The very first thing to do is to read from the ClockBound shared memory segment, take a
        // snapshot to obtain the clock parameters and bound on error, and create a timestamp.
        let cb_snap = self.clockbound_shm.snapshot()?;
        let mut clock_bound_now_result = cb_snap.now()?;

        // Now that the timestamp is created, check whether the clockbound daemon has been
        // restarted and the option to enable the clock disruption support has been turned on. If
        // so, need to create a reader for the VMClock device shared memory.
        if self.vmclock_shm.vmclock_shm_reader.is_none()
            && cb_snap.clock_disruption_support_enabled()
        {
            self.vmclock_shm.vmclock_shm_reader = Some(VMClockShmReader::new(
                self.vmclock_shm.vmclock_shm_path.as_str(),
            )?);
        }

        // Check whether the clock is disrupted. If the support to capture the clock disruption
        // signal has been explicitly disabled, there is nothing to do. Otherwise, and if the
        // VMClock shared memory is successfully read, this compares the value of the disruption
        // marker between the clockbound daemon and the VMClock. If these disagree, a disruption
        // has occured, and the clockbound daemon has not recovered from it yet.
        let is_disrupted = match self.vmclock_shm.disruption_marker()? {
            Some(marker) => marker != cb_snap.disruption_marker(),
            None => false,
        };

        // If the clock is disrupted, overwrite the status
        if is_disrupted {
            clock_bound_now_result.clock_status = ClockStatus::Disrupted;
        }

        Ok(clock_bound_now_result)
    }
}

/// `ClockBoundSHM` handles access to the shared memory segment populated by the ClockBound daemon.
struct ClockBoundSHM {
    #[allow(dead_code)]
    clockbound_shm_path: String,
    clockbound_shm_reader: ShmReader,
}

impl ClockBoundSHM {
    /// Create a new [`ClockBoundSHM`] and open the shared memory segment for reading.
    ///
    /// # Errors
    /// Returns a [`ClockErrorBound`] with an appropriate `Errno`. If the content of the segment is
    /// uninitialized, unparseable, or otherwise malformed.
    fn new(clockbound_shm_path: &str) -> Result<ClockBoundSHM, ClockBoundError> {
        // Fail early if the provided shared memory path does not exist.
        if !Path::new(clockbound_shm_path).exists() {
            let mut error = ClockBoundError::from(ShmError::SegmentNotInitialized);
            error.detail = format!(
                "Path to clockbound daemon shared memory segment does not exist: {clockbound_shm_path}"
            );
            return Err(error);
        }

        let shm_path = CString::new(clockbound_shm_path).expect("CString::new failed");
        let shm_reader = ShmReader::new(shm_path.as_c_str())?;

        Ok(ClockBoundSHM {
            clockbound_shm_path: String::from(clockbound_shm_path),
            clockbound_shm_reader: shm_reader,
        })
    }

    /// Returns a snapshot of the shared memory segment last populated by the ClockBound daemon.
    fn snapshot(&mut self) -> Result<&ClockErrorBound, ShmError> {
        self.clockbound_shm_reader.snapshot()
    }
}

/// `VMClockSHM` handles access to the shared memory segment populated by the VMClock device.
struct VMClockSHM {
    vmclock_shm_path: String,
    vmclock_shm_reader: Option<VMClockShmReader>,
}

impl VMClockSHM {
    /// Create a new [`VMClockSHM`] and open the shared memory segment for reading, if needed.
    ///
    /// # Errors
    /// Returns a [`ClockErrorBound`] with an appropriate `Errno`. If the content of the segment is
    /// uninitialized, unparseable, or otherwise malformed.
    fn new(
        vmclock_shm_path: &str,
        clock_disruption_support_enabled: bool,
    ) -> Result<VMClockSHM, ClockBoundError> {
        // Note that the support for clock disruption signal may be disabled on the ClockBound
        // daemon, in which case, no reader is created.
        let mut vmclock_shm_reader: Option<VMClockShmReader> = None;
        if clock_disruption_support_enabled {
            // Fail early if the provided shared memory path does not exist.
            if !Path::new(vmclock_shm_path).exists() {
                let mut error = ClockBoundError::from(ShmError::SegmentNotInitialized);
                error.detail = format!(
                    "Path to VMClock device shared memory segment does not exist: {vmclock_shm_path}"
                );
                return Err(error);
            }
            vmclock_shm_reader = Some(VMClockShmReader::new(vmclock_shm_path)?);
        }

        Ok(VMClockSHM {
            vmclock_shm_path: String::from(vmclock_shm_path),
            vmclock_shm_reader,
        })
    }

    /// Take a snapshot of the VMClock shared memory segment and extract the disruption marker.
    ///
    /// Note that None is returned if no SHM reader is present.
    ///
    /// # Errors
    /// Returns a [`ShmError`] if the content of the segment is uninitialized, unparseable, or
    /// otherwise malformed.
    fn disruption_marker(&mut self) -> Result<Option<u64>, ShmError> {
        if let Some(ref mut vmclock_shm_reader) = self.vmclock_shm_reader {
            let snap = vmclock_shm_reader.snapshot()?;
            return Ok(Some(snap.disruption_marker));
        }

        // The clock disruption support is not enabled
        Ok(None)
    }
}

#[derive(Debug)]
pub struct ClockBoundError {
    pub kind: ClockBoundErrorKind,
    pub errno: Errno,
    pub detail: String,
}

impl From<ShmError> for ClockBoundError {
    fn from(value: ShmError) -> Self {
        let kind = match value {
            ShmError::SyscallError(_, _) => ClockBoundErrorKind::Syscall,
            ShmError::SegmentNotInitialized => ClockBoundErrorKind::SegmentNotInitialized,
            ShmError::SegmentMalformed => ClockBoundErrorKind::SegmentMalformed,
            ShmError::CausalityBreach => ClockBoundErrorKind::CausalityBreach,
            ShmError::SegmentVersionNotSupported => ClockBoundErrorKind::SegmentVersionNotSupported,
        };

        let errno = match value {
            ShmError::SyscallError(errno, _) => errno,
            _ => Errno(0),
        };

        let detail = match value {
            ShmError::SyscallError(_, detail) => detail
                .to_str()
                .expect("Failed to convert CStr to str")
                .to_owned(),
            _ => String::from("No detail available"),
        };

        ClockBoundError {
            kind,
            errno,
            detail,
        }
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub enum ClockBoundErrorKind {
    // FIXME: the `detail` static CString is referenced on the Syscall variant. This is a temporary
    // implementation until the FFI to C is changed to have the caller allocate memory for it.
    Syscall,
    SegmentNotInitialized,
    SegmentMalformed,
    CausalityBreach,
    SegmentVersionNotSupported,
}

#[cfg(test)]
mod lib_tests {
    use super::*;
    use crate::shm::{ClockErrorBound, ShmWrite, ShmWriter};
    use crate::shm::{ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion};
    use crate::vmclock::shm::{VMClockClockStatus, VMClockShmBody};
    use crate::vmclock::shm_writer::{VMClockShmWrite, VMClockShmWriter};

    use byteorder::{NativeEndian, WriteBytesExt};
    use nix::sys::time::TimeSpec;
    use std::ffi::CStr;
    use std::fs::{File, OpenOptions};
    use std::io::Write;
    use std::path::Path;
    /// We make use of tempfile::NamedTempFile to ensure that
    /// local files that are created during a test get removed
    /// afterwards.
    use tempfile::NamedTempFile;

    // TODO: this macro is defined in more than one crate, and the code needs to be refactored to
    // remove duplication once most sections are implemented. For now, a bit of redundancy is ok to
    // avoid having to think about dependencies between crates.
    macro_rules! write_clockbound_memory_segment {
        ($file:ident,
         $magic_0:literal,
         $magic_1:literal,
         $segsize:literal,
         $version:literal,
         $generation:literal) => {
            // Build a default ClockErrorBound layout version 2
            let ceb = ClockErrorBoundGeneric::builder()
                .clock_disruption_support_enabled(true)
                .build(ClockErrorBoundLayoutVersion::V2);

            // Convert the ceb struct into a slice so we can write it all out, fairly magic.
            // Definitely needs the #[repr(C)] layout.
            let slice = unsafe {
                ::core::slice::from_raw_parts(
                    (&ceb as *const ClockErrorBound) as *const u8,
                    ::core::mem::size_of::<ClockErrorBound>(),
                )
            };

            $file
                .write_u32::<NativeEndian>($magic_0)
                .expect("Write failed magic_0");
            $file
                .write_u32::<NativeEndian>($magic_1)
                .expect("Write failed magic_1");
            $file
                .write_u32::<NativeEndian>($segsize)
                .expect("Write failed segsize");
            $file
                .write_u16::<NativeEndian>($version)
                .expect("Write failed version");
            $file
                .write_u16::<NativeEndian>($generation)
                .expect("Write failed generation");
            $file
                .write_all(slice)
                .expect("Write failed ClockErrorBound");
            $file.sync_all().expect("Sync to disk failed");
        };
    }

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

    /// Assert that VMClock can be created successfully and the disruption marker is retrieved when
    /// clock_disruption_support_enabled is true and a valid file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_enabled_success() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create and write the VMClock memory segment.
        let vmclock_shm_body = vmclockshmbody!();
        let mut vmclock_shm_writer = VMClockShmWriter::new(Path::new(&vmclock_shm_path))
            .expect("Failed to create a VMClockShmWriter");
        vmclock_shm_writer.write(&vmclock_shm_body);

        // Create the VMClock, and assert that the creation was successful.
        let vmclock_new_result = VMClockSHM::new(&vmclock_shm_path, true);
        match vmclock_new_result {
            Ok(mut vmclock) => {
                // Assert that now() does not return an error.
                let marker_result = vmclock.disruption_marker();
                assert!(marker_result.is_ok());
                assert!(marker_result.unwrap() == Some(10_u64));
            }
            Err(_) => {
                assert!(false);
            }
        }
    }

    /// Assert that VMClock will fail to be created when clock_disruption_support_enabled is true
    /// and no file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_enabled_failure() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create the VMClock, and assert that the creation was successful.
        let vmclock_new_result = VMClockSHM::new(&vmclock_shm_path, true);
        assert!(vmclock_new_result.is_err());
    }

    /// Assert that VMClock can be created successfully when clock_disruption_support_enabled is
    /// false and no file exists at the vmclock_shm_path.
    #[test]
    fn test_vmclock_now_with_clock_disruption_support_not_enabled() {
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(&vmclock_shm_path);

        // Create the VMClock, and assert that the creation was successful.
        // There should be no error even though there is no file located at vmclock_shm_path.
        let vmclock_new_result = VMClockSHM::new(&vmclock_shm_path, false);
        match vmclock_new_result {
            Ok(mut vmclock) => {
                // Assert that now() does not return an error.
                let marker_result = vmclock.disruption_marker();
                assert!(marker_result.is_ok());
                assert!(marker_result.unwrap() == None)
            }
            Err(_) => {
                assert!(false);
            }
        }
    }

    #[test]
    fn test_new_with_path_does_not_exist() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(clockbound_shm_path);
        let result = ClockBoundClient::new_with_path(clockbound_shm_path);
        assert!(result.is_err());
    }

    /// Assert that the shared memory segment can be open, read and and closed. Only a sanity test.
    #[test]
    fn test_new_with_paths_sanity_check() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_clockbound_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            800,
            0x0303,
            10
        );

        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
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
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let mut clockbound =
            match ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{:?}", e);
                    panic!("ClockBoundClient::new_with_paths() failed");
                }
            };

        let now_result = match clockbound.now() {
            Ok(result) => result,
            Err(e) => {
                eprintln!("{:?}", e);
                panic!("ClockBoundClient::now() failed");
            }
        };

        assert_eq!(now_result.clock_status, ClockStatus::Disrupted);
    }

    #[test]
    fn test_new_with_paths_does_not_exist() {
        // Test both clockbound and vmclock files do not exist.
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(vmclock_shm_path);
        let result = ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path);
        assert!(result.is_err());

        // Test clockbound file exists but vmclock file does not exist.
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        write_clockbound_memory_segment!(clockbound_shm_file, 0x414D5A4E, 0x43420200, 800, 2, 10);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_file_or_directory(vmclock_shm_path);
        let result = ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path);
        assert!(result.is_err());
        remove_file_or_directory(clockbound_shm_path);

        // Test clockbound file does not exist but vmclock file exists.
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_file_or_directory(clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
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
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let result = ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path);
        assert!(result.is_err());
    }

    /// Assert that the new() runs and returns with a ClockBoundClient if the default shared
    /// memory path exists, or with ClockBoundError if shared memory segment does not exist.
    /// We avoid writing to the shared memory for the default shared memory segment path
    /// because it is possible actual clients are relying on the ClockBound data at this location.
    #[test]
    #[ignore = "can fail if daemon has run previously with root privs"]
    fn test_new_sanity_check() {
        let result = ClockBoundClient::new();
        if Path::new(CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH).exists() {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    // FIXME: this will fail until the writer is upgraded
    // https://github.com/aws/private-clock-bound-staging/pull/158
    #[ignore = "daemon version mismatch"]
    fn test_now_clock_error_bound_now_error() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        let mut clockbound_shm_file = OpenOptions::new()
            .write(true)
            .open(clockbound_shm_path)
            .expect("open clockbound file failed");
        // Writing an older version of the shared memory segmeth, that the writer should overwrite
        write_clockbound_memory_segment!(
            clockbound_shm_file,
            0x414D5A4E,
            0x43420200,
            800,
            0x0002,
            10
        );

        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        let mut vmclock_shm_file = OpenOptions::new()
            .write(true)
            .open(vmclock_shm_path)
            .expect("open vmclock file failed");
        let vmclock_content = VMClockContent {
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
        };
        write_vmclock_content(&mut vmclock_shm_file, &vmclock_content);

        let mut writer = ShmWriter::new(
            Path::new(clockbound_shm_path),
            ClockErrorBoundLayoutVersion::V2,
            ClockErrorBoundLayoutVersion::V2,
        )
        .expect("Failed to create a writer");

        let ceb = ClockErrorBoundGeneric::builder().build(ClockErrorBoundLayoutVersion::V3);
        writer.write(&ceb);

        let mut clockbound =
            match ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{:?}", e);
                    panic!("ClockBoundClient::new_with_paths() failed");
                }
            };

        // Validate now() has a Result with a successful value.
        let now_result = clockbound.now();
        assert!(now_result.is_ok());

        // Write out data with a extremely high max_drift_ppb value so that
        // the client will have an error when calling now().
        let ceb = ClockErrorBoundGeneric::builder()
            .as_of(TimeSpec::new(100, 0))
            .void_after(TimeSpec::new(10, 0))
            .max_drift_ppb(1_000_000_000)
            .clock_status(ClockStatus::Synchronized)
            .clock_disruption_support_enabled(true)
            .build(ClockErrorBoundLayoutVersion::V3);
        writer.write(&ceb);

        // Validate now has Result with an error.
        let now_result = clockbound.now();
        assert!(now_result.is_err());
    }

    /// Test conversions from ShmError to ClockBoundError.

    #[test]
    fn test_shmerror_clockbounderror_conversion_syscallerror() {
        let errno = Errno(1);
        let detail: &CStr =
            ::std::ffi::CStr::from_bytes_with_nul("test_detail\0".as_bytes()).unwrap();
        let detail_str_slice: &str = detail.to_str().unwrap();
        let detail_string: String = detail_str_slice.to_owned();
        let shm_error = ShmError::SyscallError(errno, detail);
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(ClockBoundErrorKind::Syscall, clockbounderror.kind);
        assert_eq!(errno, clockbounderror.errno);
        assert_eq!(detail_string, clockbounderror.detail);
    }

    #[test]
    fn test_shmerror_clockbounderror_conversion_segmentnotinitialized() {
        let shm_error = ShmError::SegmentNotInitialized;
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(
            ClockBoundErrorKind::SegmentNotInitialized,
            clockbounderror.kind
        );
        assert_eq!(Errno(0), clockbounderror.errno);
        assert_eq!(String::from("No detail available"), clockbounderror.detail);
    }

    #[test]
    fn test_shmerror_clockbounderror_conversion_segmentmalformed() {
        let shm_error = ShmError::SegmentMalformed;
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(ClockBoundErrorKind::SegmentMalformed, clockbounderror.kind);
        assert_eq!(Errno(0), clockbounderror.errno);
        assert_eq!(String::from("No detail available"), clockbounderror.detail);
    }

    #[test]
    fn test_shmerror_clockbounderror_conversion_causalitybreach() {
        let shm_error = ShmError::CausalityBreach;
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(ClockBoundErrorKind::CausalityBreach, clockbounderror.kind);
        assert_eq!(Errno(0), clockbounderror.errno);
        assert_eq!(String::from("No detail available"), clockbounderror.detail);
    }
}
