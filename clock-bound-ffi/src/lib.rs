//! ClockBound Foreign Function Interface
//!
//! This crate implements the FFI for ClockBound and builds into the libclockbound library.

// Align with C naming conventions
#![allow(non_camel_case_types)]

use clock_bound_shm::{ClockStatus, ShmError, ShmReader};
use clock_bound_vmclock::shm::VMCLOCK_SHM_DEFAULT_PATH;
use clock_bound_vmclock::VMClock;
use core::ptr;
use nix::sys::time::TimeSpec;
use std::ffi::{c_char, CStr};

/// Error kind exposed over the FFI.
///
/// These have to match the C header definition.
#[repr(C)]
pub enum clockbound_err_kind {
    CLOCKBOUND_ERR_NONE,
    CLOCKBOUND_ERR_SYSCALL,
    CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED,
    CLOCKBOUND_ERR_SEGMENT_MALFORMED,
    CLOCKBOUND_ERR_CAUSALITY_BREACH,
    CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED,
}

/// Error struct exposed over the FFI.
///
/// The definition has to match the C header definition.
#[repr(C)]
pub struct clockbound_err {
    pub kind: clockbound_err_kind,
    pub errno: i32,
    pub detail: *const c_char,
}

impl Default for clockbound_err {
    fn default() -> Self {
        clockbound_err {
            kind: clockbound_err_kind::CLOCKBOUND_ERR_NONE,
            errno: 0,
            detail: ptr::null(),
        }
    }
}

impl From<ShmError> for clockbound_err {
    fn from(value: ShmError) -> Self {
        let kind = match value {
            ShmError::SyscallError(_, _) => clockbound_err_kind::CLOCKBOUND_ERR_SYSCALL,
            ShmError::SegmentNotInitialized => {
                clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_NOT_INITIALIZED
            }
            ShmError::SegmentMalformed => clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_MALFORMED,
            ShmError::CausalityBreach => clockbound_err_kind::CLOCKBOUND_ERR_CAUSALITY_BREACH,
            ShmError::SegmentVersionNotSupported => {
                clockbound_err_kind::CLOCKBOUND_ERR_SEGMENT_VERSION_NOT_SUPPORTED
            }
        };

        let errno = match value {
            ShmError::SyscallError(errno, _) => errno.0,
            _ => 0,
        };

        let detail = match value {
            ShmError::SyscallError(_, detail) => detail.as_ptr(),
            _ => ptr::null(),
        };

        clockbound_err {
            kind,
            errno,
            detail,
        }
    }
}

/// The clockbound context given to the caller.
///
/// The members of this structure are private to keep this structure opaque. The caller is not
/// meant to rely on the content of this structure, only pass it back to flex the clockbound API.
/// This allow to extend the context with extra information if needed.
pub struct clockbound_ctx {
    err: clockbound_err,
    clockbound_shm_reader: Option<ShmReader>,
    vmclock: Option<VMClock>,
}

impl clockbound_ctx {
    /// Obtain error-bounded timestamps and the ClockStatus.
    ///
    /// The result on success is a tuple of:
    /// - TimeSpec: earliest timestamp.
    /// - TimeSpec: latest timestamp.
    /// - ClockStatus: Status of the clock.
    fn now(&mut self) -> Result<(TimeSpec, TimeSpec, ClockStatus), ShmError> {
        if let Some(ref mut clockbound_shm_reader) = self.clockbound_shm_reader {
            match clockbound_shm_reader.snapshot() {
                Ok(clockerrorbound_snapshot) => clockerrorbound_snapshot.now(),
                Err(e) => Err(e),
            }
        } else if let Some(ref mut vmclock) = self.vmclock {
            vmclock.now()
        } else {
            Err(ShmError::SegmentNotInitialized)
        }
    }
}

/// Clock status exposed over the FFI.
///.
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

/// Open and create a reader to the Clockbound shared memory segment.
///
/// Create a ShmReader pointing at the path passed to this call, and package it (and any other side
/// information) into a `clockbound_ctx`. A reference to the context is passed back to the C
/// caller, and needs to live beyond the scope of this function.
///
/// # Safety
/// Rely on the caller to pass valid pointers.
///
#[no_mangle]
pub unsafe extern "C" fn clockbound_open(
    clockbound_shm_path: *const c_char,
    err: *mut clockbound_err,
) -> *mut clockbound_ctx {
    let clockbound_shm_path_cstr = CStr::from_ptr(clockbound_shm_path);
    let clockbound_shm_path = clockbound_shm_path_cstr
        .to_str()
        .expect("Failed to convert ClockBound shared memory path to str");
    let vmclock_shm_path = VMCLOCK_SHM_DEFAULT_PATH;

    let vmclock: VMClock = match VMClock::new(clockbound_shm_path, vmclock_shm_path) {
        Ok(vmclock) => vmclock,
        Err(e) => {
            if !err.is_null() {
                err.write(e.into())
            }
            return ptr::null_mut();
        }
    };

    let ctx = clockbound_ctx {
        err: Default::default(),
        clockbound_shm_reader: None,
        vmclock: Some(vmclock),
    };

    // Return the clockbound_ctx.
    //
    // The caller is responsible for calling clockbound_close() with this context which will
    // perform memory clean-up.
    return Box::leak(Box::new(ctx));
}

/// Open and create a reader to the Clockbound shared memory segment and the VMClock shared memory segment.
///
/// Create a VMClock pointing at the paths passed to this call, and package it (and any other side
/// information) into a `clockbound_ctx`. A reference to the context is passed back to the C
/// caller, and needs to live beyond the scope of this function.
///
/// # Safety
/// Rely on the caller to pass valid pointers.
#[no_mangle]
pub unsafe extern "C" fn clockbound_vmclock_open(
    clockbound_shm_path: *const c_char,
    vmclock_shm_path: *const c_char,
    err: *mut clockbound_err,
) -> *mut clockbound_ctx {
    let clockbound_shm_path_cstr = CStr::from_ptr(clockbound_shm_path);
    let clockbound_shm_path = clockbound_shm_path_cstr
        .to_str()
        .expect("Failed to convert ClockBound shared memory path to str");
    let vmclock_shm_path_cstr = CStr::from_ptr(vmclock_shm_path);
    let vmclock_shm_path = vmclock_shm_path_cstr
        .to_str()
        .expect("Failed to convert VMClock shared memory path to str");

    let vmclock: VMClock = match VMClock::new(clockbound_shm_path, vmclock_shm_path) {
        Ok(vmclock) => vmclock,
        Err(e) => {
            if !err.is_null() {
                err.write(e.into())
            }
            return ptr::null_mut();
        }
    };

    let ctx = clockbound_ctx {
        err: Default::default(),
        clockbound_shm_reader: None,
        vmclock: Some(vmclock),
    };

    // Return the clockbound_ctx.
    //
    // The caller is responsible for calling clockbound_close() with this context which will
    // perform memory clean-up.
    return Box::leak(Box::new(ctx));
}

/// Close the clockbound context.
///
/// Effectively unmap the shared memory segment and drop the ShmReader.
///
/// # Safety
///
/// Rely on the caller to pass valid pointers.
#[no_mangle]
pub unsafe extern "C" fn clockbound_close(ctx: *mut clockbound_ctx) -> *const clockbound_err {
    std::mem::drop(Box::from_raw(ctx));
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
#[inline]
#[no_mangle]
pub unsafe extern "C" fn clockbound_now(
    ctx: *mut clockbound_ctx,
    output: *mut clockbound_now_result,
) -> *const clockbound_err {
    let ctx = &mut *ctx;

    let (earliest, latest, clock_status) = match ctx.now() {
        Ok(now) => now,
        Err(e) => {
            ctx.err = e.into();
            return &ctx.err;
        }
    };

    output.write(clockbound_now_result {
        earliest: *earliest.as_ref(),
        latest: *latest.as_ref(),
        clock_status: clock_status.into(),
    });
    ptr::null()
}

#[cfg(test)]
mod t_ffi {
    use super::*;
    use clock_bound_shm::ClockErrorBound;
    use byteorder::{LittleEndian, NativeEndian, WriteBytesExt};
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
        write_clockbound_memory_segment!(clockbound_shm_file, 0x414D5A4E, 0x43420200, 800, 2, 10);

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
            let mut err: clockbound_err = Default::default();
            let mut now_result: clockbound_now_result = std::mem::zeroed();

            let ctx = clockbound_vmclock_open(
                clockbound_path_cstring.as_ptr(),
                vmclock_path_cstring.as_ptr(),
                &mut err,
            );
            assert!(!ctx.is_null());

            let errptr = clockbound_now(ctx, &mut now_result);
            assert!(errptr.is_null());
            assert_eq!(
                now_result.clock_status,
                clockbound_clock_status::CLOCKBOUND_STA_UNKNOWN
            );

            let errptr = clockbound_close(ctx);
            assert!(errptr.is_null());
        }
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
}
