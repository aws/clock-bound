// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! ClockBound Shared Memory
//!
//! This crate implements the low-level IPC functionality to share ClockErrorBound data and clock
//! status over a shared memory segment. This crate is meant to be used by the C and Rust versions
//! of the ClockBound client library.

// TODO: prevent clippy from checking for dead code. The writer module is only re-exported publicly
// if the write feature is selected. There may be a better way to do that and re-enable the lint.
#![allow(dead_code)]

// Re-exports reader and writer. The writer is conditionally included under the "writer" feature.
pub use crate::reader::ShmReader;
#[cfg(feature = "writer")]
pub use crate::writer::{ShmWrite, ShmWriter};

pub mod common;
mod reader;
mod shm_header;
mod writer;

use errno::Errno;
use nix::sys::time::{TimeSpec, TimeValLike};
use std::ffi::CStr;

use common::{clock_gettime_safe, CLOCK_MONOTONIC, CLOCK_REALTIME};

const CLOCKBOUND_RESTART_GRACE_PERIOD: TimeSpec = TimeSpec::new(5, 0);

/// Convenience macro to build a ShmError::SyscallError with extra info from errno and custom
/// origin information.
#[macro_export]
macro_rules! syserror {
    ($origin:expr) => {
        Err($crate::ShmError::SyscallError(
            ::errno::errno(),
            ::std::ffi::CStr::from_bytes_with_nul(concat!($origin, "\0").as_bytes()).unwrap(),
        ))
    };
}

/// Error condition returned by all low-level ClockBound APIs.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShmError {
    /// A system call failed.
    /// Variant includes the Errno struct with error details, and an indication on the origin of
    /// the system call that error'ed.
    SyscallError(Errno, &'static CStr),

    /// The shared memory segment is not initialized.
    SegmentNotInitialized,

    /// The shared memory segment is initialized but malformed.
    SegmentMalformed,

    /// Failed causality check when comparing timestamps
    CausalityBreach,
}

/// Definition of mutually exclusive clock status exposed to the reader.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ClockStatus {
    /// The status of the clock is unknown.
    Unknown = 0,

    /// The clock is kept accurate by the synchronization daemon.
    Synchronized = 1,

    /// The clock is free running and not updated by the synchronization daemon.
    FreeRunning = 2,
}

/// Structure that holds the ClockErrorBound data captured at a specific point in time and valid
/// until a subsequent point in time.
///
/// The ClockErrorBound structure supports calculating the actual bound on clock error at any time,
/// using its `now()` method. The internal fields are not meant to be accessed directly.
///
/// Note that the timestamps in between which this ClockErrorBound data is valid are captured using
/// a CLOCK_MONOTONIC_COARSE clock. The monotonic clock id is required to correctly measure the
/// duration during which clock drift possibly accrues, and avoid events when the clock is set,
/// smeared or affected by leap seconds.
///
/// The structure is shared across the Shared Memory segment and has a C representation to enforce
/// this specific layout.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ClockErrorBound {
    /// The CLOCK_MONOTONIC_COARSE timestamp recorded when the bound on clock error was
    /// calculated. The current implementation relies on Chrony tracking data, which accounts for
    /// the dispersion between the last clock processing event, and the reading of tracking data.
    as_of: libc::timespec,

    /// The CLOCK_MONOTONIC_COARSE timestamp beyond which the bound on clock error should not be
    /// trusted. This is a useful signal that the communication with the synchronization daemon is
    /// has failed, for example.
    void_after: libc::timespec,

    /// An absolute upper bound on the accuracy of the `CLOCK_REALTIME` clock with regards to true
    /// time at the instant represented by `as_of`.
    bound_nsec: i64,

    /// Maximum drift rate of the clock between updates of the synchronization daemon. The value
    /// stored in `bound_nsec` should increase by the following to account for the clock drift
    /// since `bound_nsec` was computed:
    /// `bound_nsec += max_drift_ppb * (now - as_of)`
    max_drift_ppb: u32,

    /// Place-holder that is reserved for future use.
    reserved1: u32,

    /// The synchronization daemon status indicates whether the daemon is synchronized,
    /// free-running, etc.
    clock_status: ClockStatus,
}

impl Default for ClockErrorBound {
    /// Get a default ClockErrorBound struct
    /// Equivalent to zero'ing this bit of memory
    fn default() -> Self {
        ClockErrorBound {
            as_of: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            void_after: libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            },
            bound_nsec: 0,
            max_drift_ppb: 0,
            reserved1: 0,
            clock_status: ClockStatus::Unknown,
        }
    }
}

impl ClockErrorBound {
    /// Create a new ClockErrorBound struct.
    pub fn new(
        as_of: libc::timespec,
        void_after: libc::timespec,
        bound_nsec: i64,
        max_drift_ppb: u32,
        reserved1: u32,
        clock_status: ClockStatus,
    ) -> ClockErrorBound {
        ClockErrorBound {
            as_of,
            void_after,
            bound_nsec,
            max_drift_ppb,
            reserved1,
            clock_status,
        }
    }

    /// The ClockErrorBound equivalent of clock_gettime(), but with bound on accuracy.
    ///
    /// Returns a pair of (earliest, latest) timespec between which current time exists. The
    /// interval width is twice the clock error bound (ceb) such that:
    ///   (earliest, latest) = ((now - ceb), (now + ceb))
    /// The function also returns a clock status to assert that the clock is being synchronized, or
    /// free-running, or ...
    pub fn now(&self) -> Result<(libc::timespec, libc::timespec, ClockStatus), ShmError> {
        // Read the clock, start with the REALTIME one to be as close as possible to the event the
        // caller is interested in. The monotonic clock should be read after. It is correct for the
        // process be preempted between the two calls: a delayed read of the monotonic clock will
        // make the bound on clock error more pessimistic, but remains correct.
        let real = clock_gettime_safe(CLOCK_REALTIME)?;
        let mono = clock_gettime_safe(CLOCK_MONOTONIC)?;

        self.compute_bound_at(real, mono)
    }

    /// Compute the bound on clock error at a given point in time.
    ///
    /// The time at which the bound is computed is defined by the (real, mono) pair of timestamps
    /// read from the realtime and monotonic clock respectively, *roughly* at the same time. The
    /// details to correctly work around the "rough" alignment of the timestamps is not something
    /// we want to leave to the user of ClockBound, hence this method is private. Although `now()`
    /// may be it only caller, decoupling the two make writing unit tests a bit easier.
    fn compute_bound_at(
        &self,
        real: libc::timespec,
        mono: libc::timespec,
    ) -> Result<(libc::timespec, libc::timespec, ClockStatus), ShmError> {
        // Take advantage of the TimeSpec implementation in the nix crate to benefit from the
        // operations on TimeSpec it implements.
        let real = TimeSpec::from(real);
        let mono = TimeSpec::from(mono);
        let as_of = TimeSpec::from(self.as_of);
        let void_after = TimeSpec::from(self.void_after);

        // Sanity checks:
        // - `now()` should operate on a consistent snapshot of the shared memory segment, and
        //   causality between mono and as_of should be enforced.
        // - a extremely high value of the `max_drift_ppb` is a sign of something going wrong
        if self.max_drift_ppb >= 1_000_000_000 {
            return Err(ShmError::SegmentMalformed);
        }

        // If the ClockErrorBound data has not been updated "recently", the status of the clock
        // cannot be guaranteed. Things are ambiguous, the synchronization daemon may be dead, or
        // its interaction with the clockbound daemon is broken, or ... In any case, we signal the
        // caller that guarantees are gone. We could return an Err here, but choosing to leverage
        // ClockStatus instead, and putting the responsibility on the caller to check the clock
        // status value being returned.
        // TODO: this may not be the most ergonomic decision, putting a pin here to revisit this
        // decision once the client code is fleshed out.
        let clock_status = match self.clock_status {
            // If the status in the shared memory segment is Unknown, returns that status.
            ClockStatus::Unknown => self.clock_status,

            // If the status is Synchronized or FreeRunning, the expectation from the client is
            // that the data is useable. However, if the clockbound daemon died or has not update
            // the shared memory segment in a while, the status written to the shared memory
            // segment may not be reliable anymore.
            ClockStatus::Synchronized | ClockStatus::FreeRunning => {
                if mono < as_of + CLOCKBOUND_RESTART_GRACE_PERIOD {
                    // Allow for a restart of the daemon, for a short period of time, the status is
                    // trusted to be correct.
                    self.clock_status
                } else if mono < void_after {
                    // Beyond the grace period, for a free running status.
                    ClockStatus::FreeRunning
                } else {
                    // If beyond void_after, no guarantee is provided anymore.
                    ClockStatus::Unknown
                }
            }
        };

        // Calculate the duration that has elapsed between the instant when the CEB parameters were
        // snapshot'ed from the SHM segment (approximated by `as_of`), and the instant when the
        // request to calculate the CEB was actually requested (approximated by `mono`). This
        // duration is used to compute the growth of the error bound due to local dispersion
        // between polling chrony and now.
        //
        // To avoid miscalculation in case the synchronization daemon is restarted, a
        // CLOCK_MONOTONIC is used, since it is designed to not jump. Because we want this to be
        // fast, and the exact accuracy is not critical here, we use CLOCK_MONOTONIC_COARSE on
        // platforms that support it.
        //
        // But ... there is a catch. When validating causality of these events that is, `as_of`
        // should always be older than `mono`, we observed this test to sometimes fail, with `mono`
        // being older by a handful of nanoseconds. The root cause is not completely understood,
        // but points to the clock resolution and/or update strategy and/or propagation of the
        // updates through the VDSO memory page. See this for details:
        // https://t.corp.amazon.com/P101954401.
        //
        // The following implementation is a mitigation.
        //   1. if as_of <= mono is younger than as_of, calculate the duration (happy path)
        //   2. if as_of - epsilon < mono < as_of, set the duration to 0
        //   3. if mono < as_of - epsilon, return an error
        //
        // In short, this relaxes the sanity check a bit to accept some imprecision in the clock
        // reading routines.
        //
        // What is a good value for `epsilon`?
        // The CLOCK_MONOTONIC_COARSE resolution is a function of the HZ kernel variable defining
        // the last kernel tick that drives this clock (e.g. HZ=250 leads to a 4 millisecond
        // resolution). We could use the `clock_getres()` system call to retrieve this value but
        // this makes diagnosing over different platform / OS configurations more complex. Instead
        // settling on an arbitrary default value of 1 millisecond.
        let causality_blur = as_of - TimeSpec::new(0, 1000);

        let duration = if mono >= as_of {
            // Happy path, no causality doubt
            mono - as_of
        } else if mono > causality_blur {
            // Causality is "almost" broken. We are within a range that could be due to the clock
            // precision. Let's approximate this to equality between mono and as_of.
            TimeSpec::new(0, 0)
        } else {
            // Causality is breached.
            return Err(ShmError::CausalityBreach);
        };

        // Inflate the bound on clock error with the maximum drift the clock may be experiencing
        // between the snapshot being read and ~now.
        let duration_sec = duration.num_nanoseconds() as f64 / 1_000_000_000_f64;
        let updated_bound = TimeSpec::nanoseconds(
            self.bound_nsec + (duration_sec * self.max_drift_ppb as f64) as i64,
        );

        // Build the (earliest, latest) interval within which true time exists.
        let earliest = real - updated_bound;
        let latest = real + updated_bound;

        Ok((*earliest.as_ref(), *latest.as_ref(), clock_status))
    }
}

#[cfg(test)]
mod t_lib {
    use super::*;

    // Convenience macro to build timespec for unit tests
    macro_rules! timespec {
        ($sec:literal, $nsec:literal) => {
            libc::timespec {
                tv_sec: $sec,
                tv_nsec: $nsec,
            }
        };
    }

    // Convenience macro to build ClockBoundError for unit tests
    macro_rules! clockbound {
        (($asof_tv_sec:literal, $asof_tv_nsec:literal), ($after_tv_sec:literal, $after_tv_nsec:literal)) => {
            ClockErrorBound {
                as_of: libc::timespec {
                    tv_sec: $asof_tv_sec,
                    tv_nsec: $asof_tv_nsec,
                },
                void_after: libc::timespec {
                    tv_sec: $after_tv_sec,
                    tv_nsec: $after_tv_nsec,
                },
                bound_nsec: 10000,   // 10 microsec
                max_drift_ppb: 1000, // 1PPM
                reserved1: 0,
                clock_status: ClockStatus::Synchronized,
            }
        };
    }

    /// Assert the bound on clock error is computed correctly
    #[test]
    fn compute_bound_ok() {
        let ceb = clockbound!((0, 0), (10, 0));
        let real = timespec!(2, 0);
        let mono = timespec!(2, 0);

        let (earliest, latest, status) = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 2 seconds have passed since the bound was snapshot, hence 2 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec, 1);
        assert_eq!(earliest.tv_nsec, 1_000_000_000 - 12_000);
        assert_eq!(latest.tv_sec, 2);
        assert_eq!(latest.tv_nsec, 12_000);
        assert_eq!(status, ClockStatus::Synchronized);
    }

    /// Assert the bound on clock error is computed correctly, with realtime and monotonic clocks
    /// disagreeing on time
    #[test]
    fn compute_bound_ok_when_real_ahead() {
        let ceb = clockbound!((0, 0), (10, 0));
        let real = timespec!(20, 0); // realtime clock way ahead
        let mono = timespec!(4, 0);

        let (earliest, latest, status) = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 4 seconds have passed since the bound was snapshot, hence 4 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec, 19);
        assert_eq!(earliest.tv_nsec, 1_000_000_000 - 14_000);
        assert_eq!(latest.tv_sec, 20);
        assert_eq!(latest.tv_nsec, 14_000);
        assert_eq!(status, ClockStatus::Synchronized);
    }

    /// Assert the clock status is FreeRunning if the ClockErrorBound data is passed the grace
    /// period
    #[test]
    fn compute_bound_force_free_running_status() {
        let ceb = clockbound!((0, 0), (100, 0));
        let real = timespec!(8, 0);
        let mono = timespec!(8, 0);

        let (earliest, latest, status) = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 8 seconds have passed since the bound was snapshot, hence 8 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec, 7);
        assert_eq!(earliest.tv_nsec, 1_000_000_000 - 18_000);
        assert_eq!(latest.tv_sec, 8);
        assert_eq!(latest.tv_nsec, 18_000);
        assert_eq!(status, ClockStatus::FreeRunning);
    }

    /// Assert the clock status is Unknown if the ClockErrorBound data is passed void_after
    #[test]
    fn compute_bound_unknown_status_if_expired() {
        let ceb = clockbound!((0, 0), (5, 0));
        let real = timespec!(10, 0);
        let mono = timespec!(10, 0); // Passed void_after

        let (earliest, latest, status) = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 10 seconds have passed since the bound was snapshot, hence 10 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec, 9);
        assert_eq!(earliest.tv_nsec, 1_000_000_000 - 20_000);
        assert_eq!(latest.tv_sec, 10);
        assert_eq!(latest.tv_nsec, 20_000);
        assert_eq!(status, ClockStatus::Unknown);
    }

    /// Assert errors are returned if the ClockBoundError data is malformed with bad drift
    #[test]
    fn compute_bound_bad_drift() {
        let mut ceb = clockbound!((0, 0), (10, 0));
        let real = timespec!(5, 0);
        let mono = timespec!(5, 0);
        ceb.max_drift_ppb = 2_000_000_000;

        assert!(ceb.compute_bound_at(real, mono).is_err());
    }

    /// Assert errors are returned if the ClockBoundError data snapshot has been taken after
    /// reading clocks at 'now'
    #[test]
    fn compute_bound_causality_break() {
        let ceb = clockbound!((5, 0), (10, 0));
        let real = timespec!(1, 0);
        let mono = timespec!(1, 0);

        let res = ceb.compute_bound_at(real, mono);

        assert!(res.is_err());
    }
}
