// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

use std::mem::MaybeUninit;

use crate::{syserror, ShmError};

pub const CLOCK_REALTIME: libc::clockid_t = libc::CLOCK_REALTIME;

// This primarily for development convenience
#[cfg(target_os = "macos")]
pub const CLOCK_MONOTONIC: libc::clockid_t = libc::CLOCK_MONOTONIC;
#[cfg(not(target_os = "macos"))]
pub const CLOCK_MONOTONIC: libc::clockid_t = libc::CLOCK_MONOTONIC_COARSE;

/// Read a specific view of time
///
/// This function wraps the `clock_gettime()` system call to conveniently return the current time
/// tracked by a specific clock. The clock_id is one of libc::CLOCK_REALTIME,
/// libc::CLOCK_MONOTONIC, etc.
pub fn clock_gettime_safe(clock_id: libc::clockid_t) -> Result<libc::timespec, ShmError> {
    // Allocate a buffer where the current time will be written to
    let mut buf: MaybeUninit<libc::timespec> = MaybeUninit::uninit();

    // SAFETY: The pointers passed to clock_gettime are valid. Assume init if the call is successful.
    unsafe {
        let ret = libc::clock_gettime(clock_id, buf.as_mut_ptr());
        if ret < 0 {
            syserror!("clock_gettime")
        } else {
            Ok(buf.assume_init())
        }
    }
}

#[cfg(test)]
mod t_common {
    use super::*;
    use nix::sys::time::TimeSpec;
    use std::{thread, time};

    /// Assert that clock_gettime(REALTIME) is functional (naive test)
    #[test]
    fn clock_gettime_safe_realtime() {
        let one = clock_gettime_safe(CLOCK_REALTIME).expect("Failed on clock_gettime");
        // Sleep a bit, some platform (macos) have a low res
        thread::sleep(time::Duration::from_millis(10));
        let two = clock_gettime_safe(CLOCK_REALTIME).expect("Failed on clock_gettime");
        let one = TimeSpec::from(one);
        let two = TimeSpec::from(two);

        assert!(two > one);
    }

    /// Assert that clock_gettime(MONOTONIC) is functional (naive test)
    #[test]
    fn clock_gettime_safe_monotonic() {
        let one = clock_gettime_safe(CLOCK_MONOTONIC).expect("Failed on clock_gettime");
        // Sleep a bit, some platform (macos) have a low res
        thread::sleep(time::Duration::from_millis(10));
        let two = clock_gettime_safe(CLOCK_MONOTONIC).expect("Failed on clock_gettime");
        let one = TimeSpec::from(one);
        let two = TimeSpec::from(two);

        assert!(two > one);
    }
}
