use crate::{syserror, ShmError};
use nix::sys::time::TimeSpec;
use nix::time::{clock_gettime, ClockId};

pub const CLOCK_REALTIME: ClockId = ClockId::CLOCK_REALTIME;

// This primarily for development convenience
#[cfg(target_os = "macos")]
pub const CLOCK_MONOTONIC: ClockId = ClockId::CLOCK_MONOTONIC;
#[cfg(not(target_os = "macos"))]
pub const CLOCK_MONOTONIC: ClockId = ClockId::CLOCK_MONOTONIC_COARSE;

/// Read a specific view of time
///
/// This function wraps the `clock_gettime()` system call to conveniently return the current time
/// tracked by a specific clock.
///
/// The clock_id is one of ClockId::CLOCK_REALTIME, ClockId::CLOCK_MONOTONIC, etc.
pub fn clock_gettime_safe(clock_id: ClockId) -> Result<TimeSpec, ShmError> {
    match clock_gettime(clock_id) {
        Ok(ts) => Ok(ts),
        _ => syserror!("clock_gettime"),
    }
}

#[cfg(test)]
mod t_common {
    use super::*;
    use std::{thread, time};

    /// Assert that clock_gettime(REALTIME) is functional (naive test)
    #[test]
    fn clock_gettime_safe_realtime() {
        let one = clock_gettime_safe(CLOCK_REALTIME).expect("Failed on clock_gettime");
        // Sleep a bit, some platform (macos) have a low res
        thread::sleep(time::Duration::from_millis(10));
        let two = clock_gettime_safe(CLOCK_REALTIME).expect("Failed on clock_gettime");

        assert!(two > one);
    }

    /// Assert that clock_gettime(MONOTONIC) is functional (naive test)
    #[test]
    fn clock_gettime_safe_monotonic() {
        let one = clock_gettime_safe(CLOCK_MONOTONIC).expect("Failed on clock_gettime");
        // Clock resolution for CLOCK_MONOTONIC_COARSE on
        // Amazon Linux 2 x86_64 on c5.4xlarge is 10 milliseconds.
        // Sleep an amount larger than that clock resolution.
        thread::sleep(time::Duration::from_millis(11));
        let two = clock_gettime_safe(CLOCK_MONOTONIC).expect("Failed on clock_gettime");

        assert!(two > one);
    }
}
