// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! This crate implements the FFI for ClockBound. It builds into the libclockbound c library that 
//! an application can use to communicate with the ClockBound daemon.
//! 
//! # Usage
//! 
//! clock-bound-ffi requires ClockBound daemon to be running to work.
//! See [ClockBound daemon documentation](../clock-bound-d/README.md) for installation instructions.
//! 
//! ## Building
//! 
//! Run the following to build the source code of this crate:
//! 
//! ```sh
//! cargo build --release
//! ```
//! 
//! It produces `libclockbound.a`, `libclockbound.so`
//! 
//! - Copy `clock-bound-ffi/include/clockbound.h` to `/usr/include/`
//! - Copy `target/release/libclockbound.a` to `/usr/lib/`
//! - Copy `target/release/libclockbound.so` to `/usr/lib/`
//! 
//! ## Example
//! 
//! Source code of a runnable c example program can be found at [../examples/c](../examples/c).
//! See the [README.md](../examples/c/README.md) in that directory for more details on how to
//! build and run the example.
//! 
//! # Updating README
//! 
//! This README is generated via [cargo-readme](https://crates.io/crates/cargo-readme). Updating can be done by running:
//! 
//! ```sh
//! cargo readme > README.md
//! ```

// Align with C naming conventions
#![allow(non_camel_case_types)]

use clock_bound_shm::{ClockErrorBound, ClockStatus, ShmError, ShmReader};
use core::ptr;
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
    reader: ShmReader,
}

impl clockbound_ctx {
    /// Request a consistent snapshot of the clockbound memory segment.
    ///
    /// This function leverages the ShmReader open when the context was created to retrieve a
    /// consistent snapshot of the bound on clock error data.
    fn snapshot(&mut self) -> Result<&ClockErrorBound, ShmError> {
        self.reader.snapshot()
    }
}

/// Clock status exposed over the FFI.
///
/// These have to match the C header definition.
#[repr(C)]
#[derive(Debug, PartialEq)]
pub enum clockbound_clock_status {
    CLOCKBOUND_STA_UNKNOWN,
    CLOCKBOUND_STA_SYNCHRONIZED,
    CLOCKBOUND_STA_FREE_RUNNING,
}

impl From<ClockStatus> for clockbound_clock_status {
    fn from(value: ClockStatus) -> Self {
        match value {
            ClockStatus::Unknown => Self::CLOCKBOUND_STA_UNKNOWN,
            ClockStatus::Synchronized => Self::CLOCKBOUND_STA_SYNCHRONIZED,
            ClockStatus::FreeRunning => Self::CLOCKBOUND_STA_FREE_RUNNING,
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
///
/// Rely on the caller to pass valid pointers.
#[no_mangle]
pub unsafe extern "C" fn clockbound_open(
    shm_path: *const c_char,
    err: *mut clockbound_err,
) -> *mut clockbound_ctx {
    let reader: ShmReader = match ShmReader::new(CStr::from_ptr(shm_path)) {
        Ok(reader) => reader,
        Err(e) => {
            if !err.is_null() {
                err.write(e.into())
            }
            return ptr::null_mut();
        }
    };

    let ctx = clockbound_ctx {
        err: Default::default(),
        reader,
    };

    // Go discover the world little context, and live forever ...
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
    let ceb_snap = match ctx.snapshot() {
        Ok(snap) => snap,
        Err(e) => {
            ctx.err = e.into();
            return &ctx.err;
        }
    };

    let (earliest, latest, clock_status) = match ceb_snap.now() {
        Ok(now) => now,
        Err(e) => {
            ctx.err = e.into();
            return &ctx.err;
        }
    };

    output.write(clockbound_now_result {
        earliest,
        latest,
        clock_status: clock_status.into(),
    });
    ptr::null()
}

#[cfg(test)]
mod t_ffi {
    /// This test module is full of side effects and create local files to test the ffi
    /// functionality. Tests run concurrently, so each test creates its own dedicated file.
    /// For now, create files in `/tmp/` and no cleaning is done.
    ///
    /// TODO: investigate how to retrieve the target-dir that would work for both brazil package
    /// and "native" cargo ones to contain artefacts better.
    ///
    /// TODO: write more / better tests
    ///
    use super::*;
    use byteorder::{NativeEndian, WriteBytesExt};
    use std::ffi::CString;
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::ffi::OsStrExt;

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

        let path_cstring =
            CString::new(std::path::Path::new(path_shm).as_os_str().as_bytes()).unwrap();
        unsafe {
            let mut err: clockbound_err = Default::default();
            let mut now_result: clockbound_now_result = std::mem::zeroed();

            let ctx = clockbound_open(path_cstring.as_ptr(), &mut err);
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
    }
}
