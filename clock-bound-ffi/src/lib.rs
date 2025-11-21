//! ClockBound Foreign Function Interface
//!
//! This crate implements the FFI for ClockBound and builds into the libclockbound library.

// Align with C naming conventions
#![allow(non_camel_case_types)]

use clock_bound::client::{ClockBoundClient, ClockBoundError, ClockBoundErrorKind};
use clock_bound::shm::{CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH, ClockBoundNowResult, ClockStatus};
use clock_bound::vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;
use core::ptr;
use errno::Errno;
use std::ffi::{CStr, CString, c_char, c_int};

/// The size of the `c_char` array passed by the C caller to populate with error information.
/// FIXME: for now this is hard-coded to match the C header definition.
const CLOCKBOUND_ERROR_DETAIL_SIZE: usize = 128;

/// Error kind exposed over the FFI.
///
/// These have to match the C header definition.
#[repr(C)]
#[derive(Debug, PartialEq)]
pub enum clockbound_err_kind {
    CLOCKBOUND_ERR_NONE,
    CLOCKBOUND_ERR_SYSCALL,
    CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED,
    CLOCKBOUND_ERR_SEGMENT_MALFORMED,
    CLOCKBOUND_ERR_CAUSALITY_BREACH,
    CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED,
}

impl From<ClockBoundErrorKind> for clockbound_err_kind {
    fn from(value: ClockBoundErrorKind) -> Self {
        match value {
            ClockBoundErrorKind::Syscall => clockbound_err_kind::CLOCKBOUND_ERR_SYSCALL,
            ClockBoundErrorKind::SegmentNotInitialized => {
                clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED
            }
            ClockBoundErrorKind::SegmentMalformed => {
                clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_MALFORMED
            }
            ClockBoundErrorKind::CausalityBreach => {
                clockbound_err_kind::CLOCKBOUND_ERR_CAUSALITY_BREACH
            }
            ClockBoundErrorKind::SegmentVersionNotSupported => {
                clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED
            }
        }
    }
}

/// Error struct exposed over the FFI.
///
/// The definition has to match the C header definition.
#[repr(C)]
pub struct clockbound_err {
    pub kind: clockbound_err_kind,
    pub errno: c_int,
    pub detail: [c_char; CLOCKBOUND_ERROR_DETAIL_SIZE],
}

impl clockbound_err {
    /// Write over the `clockbound_err` memory.
    ///
    /// This function is the interface between Rust `ClockBoundError` and its C version. The C
    /// caller allocates memory and pass a pointer to the `clockbound_err`, which is updated here.
    fn write_error(&mut self, error: ClockBoundError) {
        // Kind and errno values are set
        self.kind = clockbound_err_kind::from(error.kind);
        self.errno = error.errno.0;

        // The detail String has to be converted to a CString, ensuring it is null terminated.
        let c_string = match CString::new(error.detail) {
            Ok(c_string) => c_string,
            Err(_) => {
                // Hopefully, we never insert null terminators in the middle of an error message,
                // and should never hit that branch.
                CString::new("No detail available").unwrap()
            }
        };

        // Copy bytes including null terminator if it fits, and truncate if required.
        // Possible cast from u8 (bytes) into i8 (c_char) but is platform dependent
        let src_ptr = c_string.as_ptr();
        let dst_ptr = self.detail.as_mut_ptr();
        let copy_len = c_string
            .as_bytes_with_nul()
            .len()
            .min(CLOCKBOUND_ERROR_DETAIL_SIZE);
        // Safety: rely on the user passing a valid pointer
        unsafe {
            ptr::copy_nonoverlapping(src_ptr, dst_ptr, copy_len);
        }

        // Ensure null termination if we had to truncate.
        if copy_len >= CLOCKBOUND_ERROR_DETAIL_SIZE {
            self.detail[CLOCKBOUND_ERROR_DETAIL_SIZE - 1] = 0;
        }
    }
}

/// The clockbound context given to the caller.
///
/// The members of this structure are private to keep this structure opaque. The caller is not
/// meant to rely on the content of this structure, only pass it back to flex the clockbound API.
/// This allow to extend the context with extra information if needed.
pub struct clockbound_ctx {
    clockbound_client: ClockBoundClient,
}

impl clockbound_ctx {
    /// Obtain error-bounded timestamps and the `ClockStatus`.
    fn now(&mut self) -> Result<ClockBoundNowResult, ClockBoundError> {
        self.clockbound_client.now()
    }
}

/// Clock status exposed over the FFI.
///
#[repr(C)]
#[derive(Debug, PartialEq)]
pub enum clockbound_clock_status {
    CLOCKBOUND_STA_UNKNOWN,
    CLOCKBOUND_STA_SYNCHRONIZED,
    CLOCKBOUND_STA_FREE_RUNNING,
    CLOCKBOUND_STA_DISRUPTED,
}

impl From<ClockStatus> for clockbound_clock_status {
    fn from(value: ClockStatus) -> Self {
        match value {
            ClockStatus::Unknown => Self::CLOCKBOUND_STA_UNKNOWN,
            ClockStatus::Synchronized => Self::CLOCKBOUND_STA_SYNCHRONIZED,
            ClockStatus::FreeRunning => Self::CLOCKBOUND_STA_FREE_RUNNING,
            ClockStatus::Disrupted => Self::CLOCKBOUND_STA_DISRUPTED,
        }
    }
}

/// Result of the `clockbound_now()` function exposed over the FFI.
///
/// These have to match the C header definition.
#[repr(C)]
pub struct clockbound_now_result {
    earliest: libc::timespec,
    latest: libc::timespec,
    clock_status: clockbound_clock_status,
}

impl From<ClockBoundNowResult> for clockbound_now_result {
    fn from(value: ClockBoundNowResult) -> Self {
        let ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        } = value;

        Self {
            earliest: *earliest.as_ref(),
            latest: *latest.as_ref(),
            clock_status: clock_status.into(),
        }
    }
}

/// Open and create a reader to the Clockbound shared memory segment.
///
/// Create a `ShmReader` pointing at the path passed to this call, and package it (and any other side
/// information) into a `clockbound_ctx`. A reference to the context is passed back to the C
/// caller, and needs to live beyond the scope of this function.
///
/// # Safety
/// Rely on the caller to pass valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clockbound_open(err: *mut clockbound_err) -> *mut clockbound_ctx {
    // Safety: Convert the default path to a CString, and then an array of bytes. The conversion of
    // the default path into a CString is unit tested and safe.
    #[expect(clippy::missing_panics_doc, reason = "infallible")]
    unsafe {
        let clockbound_shm_path = CString::new(CLOCKBOUND_SHM_CLIENT_DEFAULT_PATH).unwrap();
        let vmclock_shm_path = CString::new(VMCLOCK_SHM_DEFAULT_PATH).unwrap();
        clockbound_open_with(clockbound_shm_path.as_ptr(), vmclock_shm_path.as_ptr(), err)
    }
}

/// Open and create a reader to the Clockbound shared memory segment and the VMClock shared memory segment.
///
/// Create a VMClock pointing at the paths passed to this call, and package it (and any other side
/// information) into a `clockbound_ctx`. A reference to the context is passed back to the C
/// caller, and needs to live beyond the scope of this function.
///
/// # Safety
/// Rely on the caller to pass valid pointers.
///
// # TODO
// Currently handles errors when converting paths provided by the caller, but it is a bit of a
// stretch to map a UTF conversion error to a `SegmentNotInitialized` kind. A better option would
// be to have a more meaningful variant added to `ClockBoundErrorKind` but don't want to change the
// C API just yet.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clockbound_open_with(
    clockbound_shm_path: *const c_char,
    vmclock_shm_path: *const c_char,
    err: *mut clockbound_err,
) -> *mut clockbound_ctx {
    // Safety: Rely on caller to pass valid pointers
    let clockbound_shm_path_cstr = unsafe { CStr::from_ptr(clockbound_shm_path) };
    let clockbound_shm_path = match clockbound_shm_path_cstr.to_str() {
        Ok(path) => path,
        Err(e) => {
            let cb_err = ClockBoundError {
                kind: ClockBoundErrorKind::SegmentNotInitialized,
                errno: Errno(22_i32), // EINVAL, Invalid argument.
                detail: format!("Failed to convert ClockBound shared memory path to str: {e}"),
            };
            unsafe {
                // Safety: rely on caller to pass valid pointers
                if let Some(rust_err) = err.as_mut() {
                    rust_err.write_error(cb_err);
                }
            }
            return ptr::null_mut();
        }
    };

    // Safety: Rely on caller to pass valid pointers
    let vmclock_shm_path_cstr = unsafe { CStr::from_ptr(vmclock_shm_path) };
    let vmclock_shm_path = match vmclock_shm_path_cstr.to_str() {
        Ok(path) => path,
        Err(e) => {
            let cb_err = ClockBoundError {
                kind: ClockBoundErrorKind::SegmentNotInitialized,
                errno: Errno(22_i32), // EINVAL, Invalid argument.
                detail: format!("Failed to convert VMClock shared memory path to str: {e}"),
            };

            unsafe {
                // Safety: rely on caller to pass valid pointers
                if let Some(rust_err) = err.as_mut() {
                    rust_err.write_error(cb_err);
                }
            }
            return ptr::null_mut();
        }
    };

    let clockbound_client =
        match ClockBoundClient::new_with_paths(clockbound_shm_path, vmclock_shm_path) {
            Ok(client) => client,
            Err(cb_err) => {
                unsafe {
                    // Safety: rely on caller to pass valid pointers
                    if let Some(rust_err) = err.as_mut() {
                        rust_err.write_error(cb_err);
                    }
                }
                return ptr::null_mut();
            }
        };

    // Return the clockbound_ctx. The caller is responsible for calling clockbound_close() with
    // this context which will perform memory clean-up.
    let ctx = clockbound_ctx { clockbound_client };
    Box::leak(Box::new(ctx))
}

/// Close the clockbound context.
///
/// Effectively unmap the shared memory segment and drop the `ShmReader`.
///
/// # Safety
///
/// Rely on the caller to pass valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clockbound_close(
    ctx: *mut clockbound_ctx,
    err: *mut clockbound_err,
) -> *const clockbound_err {
    if ctx.is_null() {
        let cb_err = ClockBoundError {
            kind: ClockBoundErrorKind::SegmentNotInitialized,
            errno: Errno(22_i32), // EINVAL, Invalid argument.
            detail: "Cannot close a NULL context".to_string(),
        };

        // Safety: rely on caller to pass valid pointers
        unsafe {
            if let Some(rust_err) = err.as_mut() {
                rust_err.write_error(cb_err);
                return err;
            }
        }
        return ptr::null();
    }

    // Safety: Rely on caller to pass valid pointers
    std::mem::drop(unsafe { Box::from_raw(ctx) });
    ptr::null()
}

/// Call to the `now()` operation of the ClockBound API.
///
/// Grab the most up to date data defining the clock error bound CEB, read the current time C(t)
/// from the system clock and returns the interval [(C(t) - CEB), (C(t) + CEB)] in which true time
/// exists. The call also populate an enum with the underlying clock status.
///
/// # Safety
///
/// Have no choice but rely on the caller to pass valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn clockbound_now(
    ctx: *mut clockbound_ctx,
    output: *mut clockbound_now_result,
    err: *mut clockbound_err,
) -> *const clockbound_err {
    // Safety: Rely on caller to pass valid pointers
    let ctx = unsafe { &mut *ctx };

    // Get earliest and latest timestamps, as well as the clock status
    let cb_now = match ctx.now() {
        Ok(now) => now,
        Err(cb_err) => {
            unsafe {
                // Safety: rely on caller to pass valid pointers
                if let Some(rust_err) = err.as_mut() {
                    rust_err.write_error(cb_err);
                }
            }
            return err;
        }
    };

    // Safety: Rely on caller to pass valid pointers
    unsafe {
        output.write(clockbound_now_result::from(cb_now));
    }
    ptr::null()
}

#[cfg(test)]
mod t_ffi {
    use super::*;
    use byteorder::{LittleEndian, NativeEndian, WriteBytesExt};
    use clock_bound::shm::{ClockErrorBound, ClockErrorBoundGeneric, ClockErrorBoundLayoutVersion};
    use std::ffi::CString;
    use std::fs::OpenOptions;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;
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
            // Build a default ClockErrorBound struct (layout version 2)
            let ceb = ClockErrorBoundGeneric::builder().build(ClockErrorBoundLayoutVersion::V2);

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

    macro_rules! write_vmclock_shm_header {
        ($file:ident,
         $magic:literal,
         $size:literal,
         $version:literal,
         $counter_id:literal,
         $time_type:literal,
         $seq_count:literal) => {
            $file
                .write_u32::<LittleEndian>($magic)
                .expect("Write failed magic");
            $file
                .write_u32::<LittleEndian>($size)
                .expect("Write failed size");
            $file
                .write_u16::<LittleEndian>($version)
                .expect("Write failed version");
            $file
                .write_u8($counter_id)
                .expect("Write failed counter_id");
            $file.write_u8($time_type).expect("Write failed time_type");
            $file
                .write_u32::<LittleEndian>($seq_count)
                .expect("Write failed seq_count");
            $file.sync_all().expect("Sync to disk failed");
        };
    }

    /// Assert that the clockbound_err is updated with the correct values.
    #[test]
    fn test_clockbound_err_write_from() {
        let cb_error = ClockBoundError {
            kind: ClockBoundErrorKind::Syscall,
            errno: Errno(42),
            detail: String::from("Something weird happened"),
        };
        let mut err: clockbound_err = unsafe { std::mem::zeroed() };
        let raw_ptr = &mut err;

        // Write the ClockBoundError onto the clockbound_err
        raw_ptr.write_error(cb_error);

        assert_eq!(err.kind, clockbound_err_kind::CLOCKBOUND_ERR_SYSCALL);
        assert_eq!(err.errno, 42 as c_int);

        let c_char_ptr: *const c_char = err.detail.as_ptr();
        let c_str: &CStr = unsafe { CStr::from_ptr(c_char_ptr) };
        let c_string: CString = CString::from(c_str);
        assert_eq!(c_string, CString::new("Something weird happened").unwrap());
    }

    /// Assert that the clockbound_err is updated with the correct values, but truncate message
    #[test]
    fn test_clockbound_err_write_from_truncated() {
        let cb_error = ClockBoundError {
            kind: ClockBoundErrorKind::SegmentMalformed,
            errno: Errno(42),
            detail: "a".repeat(2 * CLOCKBOUND_ERROR_DETAIL_SIZE),
        };
        let mut err: clockbound_err = unsafe { std::mem::zeroed() };
        let raw_ptr = &mut err;

        // Write the ClockBoundError onto the clockbound_err
        raw_ptr.write_error(cb_error);

        assert_eq!(
            err.kind,
            clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_MALFORMED
        );
        assert_eq!(err.errno, 42 as c_int);

        for c in 1..(CLOCKBOUND_ERROR_DETAIL_SIZE - 1) {
            assert_eq!(err.detail[c], 'a' as c_char);
        }
        assert_eq!(err.detail[CLOCKBOUND_ERROR_DETAIL_SIZE - 1], 0);
    }

    /// Assert that the clock status is converted correctly between representations. This is a bit
    /// of a "useless" unit test since it mimics the code closely. However, this is a core
    /// property we give to the callers, so may as well.
    #[test]
    fn test_clock_status_conversion() {
        assert_eq!(
            clockbound_clock_status::from(ClockStatus::Unknown),
            clockbound_clock_status::CLOCKBOUND_STA_UNKNOWN
        );
        assert_eq!(
            clockbound_clock_status::from(ClockStatus::Synchronized),
            clockbound_clock_status::CLOCKBOUND_STA_SYNCHRONIZED
        );
        assert_eq!(
            clockbound_clock_status::from(ClockStatus::FreeRunning),
            clockbound_clock_status::CLOCKBOUND_STA_FREE_RUNNING
        );
        assert_eq!(
            clockbound_clock_status::from(ClockStatus::Disrupted),
            clockbound_clock_status::CLOCKBOUND_STA_DISRUPTED
        );
    }

    /// Assert that the shared memory segment can be open, read and and closed. Only a sanity test.
    #[test]
    fn test_clockbound_vmclock_open_sanity_check() {
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
        write_vmclock_shm_header!(
            vmclock_shm_file,
            0x4B4C4356,
            104_u32,
            1_u16,
            0_u8,
            0_u8,
            0_u32
        );

        let clockbound_path_cstring = CString::new(
            std::path::Path::new(clockbound_shm_path)
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();
        let vmclock_path_cstring = CString::new(
            std::path::Path::new(vmclock_shm_path)
                .as_os_str()
                .as_bytes(),
        )
        .unwrap();

        unsafe {
            let mut err: clockbound_err = std::mem::zeroed();
            let mut now_result: clockbound_now_result = std::mem::zeroed();

            let ctx = clockbound_open_with(
                clockbound_path_cstring.as_ptr(),
                vmclock_path_cstring.as_ptr(),
                &mut err,
            );
            assert!(!ctx.is_null());

            let errptr = clockbound_now(ctx, &mut now_result, &mut err);
            assert!(errptr.is_null());
            assert_eq!(
                now_result.clock_status,
                clockbound_clock_status::CLOCKBOUND_STA_UNKNOWN
            );

            let errptr = clockbound_close(ctx, &mut err);
            assert!(errptr.is_null());
        }
    }

    /// Assert that closing the context survives being passed a NULL pointer.
    #[test]
    fn test_clockbound_close() {
        // Should not crash on everything being NULL
        unsafe {
            assert_eq!(
                ptr::null(),
                clockbound_close(ptr::null_mut(), ptr::null_mut())
            );
        }

        // ... and should get some info if an error is provided
        unsafe {
            let mut err: clockbound_err = std::mem::zeroed();
            let raw_err_ptr: *const clockbound_err = &err;
            assert_eq!(raw_err_ptr, clockbound_close(ptr::null_mut(), &mut err));

            let c_char_ptr: *const c_char = err.detail.as_ptr();
            let c_str: &CStr = CStr::from_ptr(c_char_ptr);
            let c_string: CString = CString::from(c_str);
            assert_eq!(
                c_string,
                CString::new("Cannot close a NULL context").unwrap()
            );
        }
    }
}
