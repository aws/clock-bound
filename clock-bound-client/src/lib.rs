//! A client library to communicate with ClockBound daemon. This client library is written in pure Rust.
//!
pub use clock_bound_shm::ClockStatus;
use clock_bound_shm::ShmError;
pub use clock_bound_vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;
use clock_bound_vmclock::VMClock;
use errno::Errno;
use nix::sys::time::TimeSpec;
use std::path::Path;

pub const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm0";

pub struct ClockBoundClient {
    vmclock: VMClock,
}

impl ClockBoundClient {
    /// Creates and returns a new ClockBoundClient.
    ///
    /// The creation process also initializes a shared memory reader
    /// with the shared memory default path that is used by
    /// the ClockBound daemon.
    ///
    pub fn new() -> Result<ClockBoundClient, ClockBoundError> {
        // Validate that the default ClockBound shared memory path exists.
        if !Path::new(CLOCKBOUND_SHM_DEFAULT_PATH).exists() {
            let mut error = ClockBoundError::from(ShmError::SegmentNotInitialized);
            error.detail = String::from(
                "Default path for the ClockBound shared memory segment does not exist: ",
            );
            error.detail.push_str(CLOCKBOUND_SHM_DEFAULT_PATH);
            return Err(error);
        }

        // Create a ClockBoundClient that makes use of the ClockBound daemon and VMClock.
        //
        // Clock disruption is expected to be handled by ClockBound daemon
        // in coordination with this VMClock.
        let vmclock = VMClock::new(CLOCKBOUND_SHM_DEFAULT_PATH, VMCLOCK_SHM_DEFAULT_PATH)?;

        Ok(ClockBoundClient { vmclock })
    }

    /// Creates and returns a new ClockBoundClient, specifying a shared
    /// memory path that is being used by the ClockBound daemon.
    /// The VMClock will be accessed by reading the default VMClock
    /// shared memory path.
    pub fn new_with_path(clockbound_shm_path: &str) -> Result<ClockBoundClient, ClockBoundError> {
        // Validate that the provided ClockBound shared memory path exists.
        if !Path::new(clockbound_shm_path).exists() {
            let mut error = ClockBoundError::from(ShmError::SegmentNotInitialized);
            error.detail = String::from("Path in argument `clockbound_shm_path` does not exist: ");
            error.detail.push_str(clockbound_shm_path);
            return Err(error);
        }

        // Create a ClockBoundClient that makes use of the ClockBound daemon and VMClock.
        //
        // Clock disruption is expected to be handled by ClockBound daemon
        // in coordination with this VMClock.
        let vmclock = VMClock::new(clockbound_shm_path, VMCLOCK_SHM_DEFAULT_PATH)?;

        Ok(ClockBoundClient { vmclock })
    }

    /// Creates and returns a new ClockBoundClient, specifying a shared
    /// memory paths that are being used by the ClockBound daemon and by the VMClock,
    /// respectively.
    pub fn new_with_paths(
        clockbound_shm_path: &str,
        vmclock_shm_path: &str,
    ) -> Result<ClockBoundClient, ClockBoundError> {
        // Validate that the provided shared memory paths exists.
        if !Path::new(clockbound_shm_path).exists() {
            let mut error = ClockBoundError::from(ShmError::SegmentNotInitialized);
            error.detail = String::from("Path in argument `clockbound_shm_path` does not exist: ");
            error.detail.push_str(clockbound_shm_path);
            return Err(error);
        }

        let vmclock = VMClock::new(clockbound_shm_path, vmclock_shm_path)?;

        Ok(ClockBoundClient { vmclock })
    }

    /// Obtains the clock error bound and clock status at the current moment.
    pub fn now(&mut self) -> Result<ClockBoundNowResult, ClockBoundError> {
        let (earliest, latest, clock_status) = self.vmclock.now()?;

        Ok(ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        })
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub enum ClockBoundErrorKind {
    Syscall,
    SegmentNotInitialized,
    SegmentMalformed,
    CausalityBreach,
    SegmentVersionNotSupported,
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
            _ => String::new(),
        };

        ClockBoundError {
            kind,
            errno,
            detail,
        }
    }
}

/// Result of the `ClockBoundClient::now()` function.
#[derive(PartialEq, Clone, Debug)]
pub struct ClockBoundNowResult {
    pub earliest: TimeSpec,
    pub latest: TimeSpec,
    pub clock_status: ClockStatus,
}

#[cfg(test)]
mod lib_tests {
    use super::*;
    use clock_bound_shm::{ClockErrorBound, ShmWrite, ShmWriter};
    use clock_bound_vmclock::shm::VMClockClockStatus;
    use byteorder::{NativeEndian, WriteBytesExt};
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
            // Build a the bound on clock error data
            let ceb = ClockErrorBound::new(
                TimeSpec::new(0, 0),  // as_of
                TimeSpec::new(0, 0),  // void_after
                0,                    // bound_nsec
                0,                    // disruption_marker
                0,                    // max_drift_ppb
                ClockStatus::Unknown, // clock_status
                true,                 // clock_disruption_support_enabled
            );

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

    fn remove_path_if_exists(path_shm: &str) {
        let path = Path::new(path_shm);
        if path.exists() {
            if path.is_dir() {
                std::fs::remove_dir_all(path_shm).expect("failed to remove file");
            } else {
                std::fs::remove_file(path_shm).expect("failed to remove file");
            }
        }
    }

    #[test]
    fn test_new_with_path_does_not_exist() {
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);
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
        write_clockbound_memory_segment!(clockbound_shm_file, 0x414D5A4E, 0x43420200, 800, 2, 10);

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

        assert_eq!(now_result.clock_status, ClockStatus::Unknown);
    }

    #[test]
    fn test_new_with_paths_does_not_exist() {
        // Test both clockbound and vmclock files do not exist.
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);
        let vmclock_shm_tempfile = NamedTempFile::new().expect("create vmclock file failed");
        let vmclock_shm_temppath = vmclock_shm_tempfile.into_temp_path();
        let vmclock_shm_path = vmclock_shm_temppath.to_str().unwrap();
        remove_path_if_exists(vmclock_shm_path);
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
        remove_path_if_exists(vmclock_shm_path);
        let result = ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path);
        assert!(result.is_err());
        remove_path_if_exists(clockbound_shm_path);

        // Test clockbound file does not exist but vmclock file exists.
        let clockbound_shm_tempfile = NamedTempFile::new().expect("create clockbound file failed");
        let clockbound_shm_temppath = clockbound_shm_tempfile.into_temp_path();
        let clockbound_shm_path = clockbound_shm_temppath.to_str().unwrap();
        remove_path_if_exists(clockbound_shm_path);
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
    fn test_new_sanity_check() {
        let result = ClockBoundClient::new();
        if Path::new(CLOCKBOUND_SHM_DEFAULT_PATH).exists() {
            assert!(result.is_ok());
        } else {
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_now_clock_error_bound_now_error() {
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

        let mut writer =
            ShmWriter::new(Path::new(clockbound_shm_path)).expect("Failed to create a writer");

        let ceb = ClockErrorBound::new(
            TimeSpec::new(0, 0),  // as_of
            TimeSpec::new(0, 0),  // void_after
            0,                    // bound_nsec
            0,                    // disruption_marker
            0,                    // max_drift_ppb
            ClockStatus::Unknown, // clock_status
            true,                 // clock_disruption_support_enabled
        );
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
        let ceb = ClockErrorBound::new(
            TimeSpec::new(100, 0),
            TimeSpec::new(10, 0),
            0,
            0,
            1_000_000_000, // max_drift_ppb
            ClockStatus::Synchronized,
            true,
        );
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
        assert_eq!(String::new(), clockbounderror.detail);
    }

    #[test]
    fn test_shmerror_clockbounderror_conversion_segmentmalformed() {
        let shm_error = ShmError::SegmentMalformed;
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(ClockBoundErrorKind::SegmentMalformed, clockbounderror.kind);
        assert_eq!(Errno(0), clockbounderror.errno);
        assert_eq!(String::new(), clockbounderror.detail);
    }

    #[test]
    fn test_shmerror_clockbounderror_conversion_causalitybreach() {
        let shm_error = ShmError::CausalityBreach;
        // Perform the conversion.
        let clockbounderror = ClockBoundError::from(shm_error);
        assert_eq!(ClockBoundErrorKind::CausalityBreach, clockbounderror.kind);
        assert_eq!(Errno(0), clockbounderror.errno);
        assert_eq!(String::new(), clockbounderror.detail);
    }
}
