// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0


//! A client library to communicate with ClockBound daemon. This client library is written in pure Rust.
//!
//! # Usage
//!
//! The ClockBound client library requires ClockBound daemon to be running to work.
//! See [ClockBound daemon documentation](../clock-bound-d/README.md) for installation instructions.
//!
//! For Rust programs built with Cargo, add "clock-bound-client" as a dependency in your Cargo.toml.
//!
//! For example:
//!
//! ```text
//! [dependencies]
//! clock-bound-client = "1.0.0"
//! ```
//!
//! ## Examples
//!
//! Source code of a runnable example program can be found at [../examples/rust](../examples/rust).
//! See the [README.md](../examples/rust/README.md) in that directory for more details on how to
//! build and run the example.
//!
//! ## Building
//!
//! Run the following to build the source code of this crate:
//!
//! ```text
//! cargo build
//! ```
//!
//! # Updating README
//!
//! This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
//!
//! ```text
//! cargo readme > README.md
//! ```
use std::ffi::CString;
use nix::sys::time::TimeSpec;
use errno::Errno;
use clock_bound_shm::{ShmError, ShmReader};
pub use clock_bound_shm::ClockStatus;

pub const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm";

pub struct ClockBoundClient {
    reader: ShmReader,
}

impl ClockBoundClient {
    /// Creates and returns a new ClockBoundClient.
    ///
    /// The creation process also initializes a shared memory reader
    /// with the shared memory default path that is used by
    /// the ClockBound daemon.
    ///
    pub fn new() -> Result<ClockBoundClient, ClockBoundError> {
        ClockBoundClient::new_with_path(CLOCKBOUND_SHM_DEFAULT_PATH)
    }

    /// Creates and returns a new ClockBoundClient, specifying a shared
    /// memory path that is being used by the ClockBound daemon.
    pub fn new_with_path(shm_path: &str) -> Result<ClockBoundClient, ClockBoundError> {
        let shm_path = CString::new(shm_path).expect("CString::new failed");

        let reader: ShmReader = match ShmReader::new(shm_path.as_c_str()) {
            Ok(reader) => reader,
            Err(e) => {
                return Err(ClockBoundError::from(e));
            }
        };

        Ok(ClockBoundClient { reader })
    }

    /// Obtains the clock error bound and clock status at the current moment.
    pub fn now(&mut self) -> Result<ClockBoundNowResult, ClockBoundError> {
        let ceb_snap = match self.reader.snapshot() {
            Ok(snap) => snap,
            Err(e) => {
                return Err(ClockBoundError::from(e));
            }
        };

        let (earliest, latest, clock_status) = match ceb_snap.now() {
            Ok(now) => now,
            Err(e) => {
                return Err(ClockBoundError::from(e));
            }
        };

        Ok(ClockBoundNowResult {
            earliest: TimeSpec::from(earliest),
            latest: TimeSpec::from(latest),
            clock_status: clock_status,
        })
    }
}

#[derive(Hash, PartialEq, Eq, Clone, Debug)]
pub enum ClockBoundErrorKind {
    None,
    Syscall,
    SegmentNotInitialized,
    SegmentMalformed,
    CausalityBreach,
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
        };

        let errno = match value {
            ShmError::SyscallError(errno, _) => errno,
            _ => Errno(0),
        };

        let detail = match value {
            ShmError::SyscallError(_, detail) => detail.to_str().expect("Failed to convert CStr to str").to_owned(),
            _ => String::new(),
        };

        ClockBoundError {
            kind,
            errno,
            detail,
        }
    }
}

/// Result of the `clockbound_now()` function.
#[derive(PartialEq, Clone, Debug)]
pub struct ClockBoundNowResult {
    pub earliest: TimeSpec,
    pub latest: TimeSpec,
    pub clock_status: ClockStatus,
}

#[cfg(test)]
mod lib_tests {
    /// This test module is full of side effects and create local files to test the ffi
    /// functionality. Tests run concurrently, so each test creates its own dedicated file.
    /// For now, create files in `/tmp/` and no cleaning is done.
    ///
    /// TODO: write more / better tests
    ///
    use super::*;
    use byteorder::{NativeEndian, WriteBytesExt};
    use std::fs::File;
    use std::io::Write;
    use std::ffi::CStr;
    use clock_bound_shm::ClockErrorBound;

    // TODO: this macro is defined in more than one crate, and the code needs to be refactored to
    // remove duplication once most sections are implemented. For now, a bit of redundancy is ok to
    // avoid having to think about dependencies between crates.
    macro_rules! write_memory_segment {
        ($file:ident,
         $magic_0:literal,
         $magic_1:literal,
         $segsize:literal,
         $version:literal,
         $generation:literal) => {
            // Build a the bound on clock error data
            let ceb = ClockErrorBound::default();

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

    /// Assert that the shared memory segment can be open, read and and closed. Only a sanity test.
    #[test]
    fn test_sanity_check() {
        let path_shm = "/tmp/test_ffi";
        let mut file = File::create(path_shm).expect("create file failed");
        write_memory_segment!(file, 0x414D5A4E, 0x43420200, 800, 3, 10);

        let mut clockbound = match ClockBoundClient::new_with_path(path_shm) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{:?}", e);
                panic!("ClockBoundClient::new_with_path() failed");
            },
        };

        let now_result = match clockbound.now() {
            Ok(result) => result,
            Err(e) => {
                eprintln!("{:?}", e);
                panic!("ClockBoundClient::now() failed");
            },
        };

        assert_eq!(now_result.clock_status, ClockStatus::Unknown);
    }

    /// Test conversions from ShmError to ClockBoundError.

    #[test]
    fn test_shmerror_clockbounderror_conversion_syscallerror() {
        let errno = Errno(1);
        let detail: &CStr = ::std::ffi::CStr::from_bytes_with_nul("test_detail\0".as_bytes()).unwrap();
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
        assert_eq!(ClockBoundErrorKind::SegmentNotInitialized, clockbounderror.kind);
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
