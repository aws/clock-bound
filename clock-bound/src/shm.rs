//! ClockBound Shared Memory
//!
//! This crate implements the low-level IPC functionality to share `ClockErrorBound` data and clock
//! status over a shared memory segment. This crate is meant to be used by the C and Rust versions
//! of the ClockBound client library.

// TODO: prevent clippy from checking for dead code. The writer module is only re-exported publicly
// if the write feature is selected. There may be a better way to do that and re-enable the lint.
#![allow(dead_code)]

pub mod common;
mod reader;
mod shm_header;
mod writer;

// Re-exports reader and writer. The writer is conditionally included under the "writer" feature.
pub use reader::ShmReader;
pub use writer::{ShmWrite, ShmWriter};

use bon::Builder;
use errno::Errno;
use nix::sys::time::{TimeSpec, TimeValLike};
use std::error::Error;
use std::ffi::CStr;
use std::fmt;

use common::{CLOCK_MONOTONIC, CLOCK_REALTIME, clock_gettime_safe};

pub const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm0";

const CLOCKBOUND_RESTART_GRACE_PERIOD: TimeSpec = TimeSpec::new(5, 0);

/// Convenience macro to build a `ShmError::SyscallError` with extra info from errno and custom
/// origin information.
#[macro_export]
macro_rules! syserror {
    ($origin:expr) => {
        Err($crate::shm::ShmError::SyscallError(
            ::errno::errno(),
            ::std::ffi::CStr::from_bytes_with_nul(concat!($origin, "\0").as_bytes()).unwrap(),
        ))
    };
}

pub trait ClockBoundSnapshot {
    /// The `ClockErrorBound` equivalent of `clock_gettime()`, but with bound on accuracy.
    ///
    /// Returns a `ClockBoundNowResult` with contains the (earliest, latest) timespec between which
    /// current time exists. The interval width is twice the clock error bound (ceb) such that:
    ///   (earliest, latest) = ((now - ceb), (now + ceb))
    ///
    /// The function also returns a clock status to assert that the clock is being synchronized, or
    /// free-running, or ...
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    fn now(&self) -> Result<ClockBoundNowResult, ShmError>;
}

/// Enum that holds supported layout of the `ClockErrorBound` stored in the ClockBound daemon
/// shared memory segment.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ClockErrorBound {
    V2(ClockErrorBoundV2),
}

impl ClockErrorBound {
    pub fn as_of(&self) -> TimeSpec {
        match self {
            ClockErrorBound::V2(ceb) => ceb.as_of,
        }
    }

    pub fn void_after(&self) -> TimeSpec {
        match self {
            ClockErrorBound::V2(ceb) => ceb.void_after,
        }
    }

    pub fn bound_nsec(&self) -> i64 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.bound_nsec,
        }
    }

    pub fn max_drift_ppb(&self) -> u32 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.max_drift_ppb,
        }
    }

    pub fn clock_status(&self) -> ClockStatus {
        match self {
            ClockErrorBound::V2(ceb) => ceb.clock_status,
        }
    }

    pub fn disruption_marker(&self) -> u64 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.disruption_marker,
        }
    }

    pub fn clock_disruption_support_enabled(&self) -> bool {
        match self {
            ClockErrorBound::V2(ceb) => ceb.clock_disruption_support_enabled,
        }
    }
}

impl ClockBoundSnapshot for ClockErrorBound {
    fn now(&self) -> Result<ClockBoundNowResult, ShmError> {
        match self {
            ClockErrorBound::V2(ceb) => ceb.now(),
        }
    }
}

/// Generic `ClockErrorBound` builder.
#[derive(Builder)]
// Rename auto-generated build() function into build_internal so we have a custom finishing
// function to create the enum variants
#[builder(finish_fn(vis = "", name = build_internal))]
pub struct ClockErrorBoundGeneric {
    #[builder(default = TimeSpec::new(0, 0))]
    as_of: TimeSpec,

    #[builder(default = TimeSpec::new(0, 0))]
    void_after: TimeSpec,

    #[builder(default)]
    bound_nsec: i64,

    #[builder(default)]
    disruption_marker: u64,

    #[builder(default)]
    max_drift_ppb: u32,

    #[builder(default = ClockStatus::Unknown)]
    clock_status: ClockStatus,

    #[builder(default)]
    clock_disruption_support_enabled: bool,
}

impl<S: clock_error_bound_generic_builder::IsComplete> ClockErrorBoundGenericBuilder<S> {
    /// Custom `build` finishing function on the generated `ClockErrorBoundLayoutBuilder`.
    ///
    /// Take the layout version number as a parameter, it is a u16 to ease casting of the earlier
    /// version of the `SHMHeader`.
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    pub fn build(self, layout_version: u16) -> Result<ClockErrorBound, ShmError> {
        // Build the ClockErrorBoundGeneric object
        let ceb = self.build_internal();

        // Build the specific version of the ClockErrorBound
        match layout_version {
            2 => Ok(ClockErrorBound::V2(ClockErrorBoundV2::new(
                ceb.as_of,
                ceb.void_after,
                ceb.bound_nsec,
                ceb.disruption_marker,
                ceb.max_drift_ppb,
                ceb.clock_status,
                ceb.clock_disruption_support_enabled,
            ))),
            _ => Err(ShmError::SegmentVersionNotSupported),
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

/// Error condition returned by all low-level ClockBound APIs.
///
// FIXME: the `detail` static CString referenced on the Syscall variant can be changed. The C
// caller now pass memory over the FFI interface.
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

    /// Failed causality check when comparing timestamps.
    CausalityBreach,

    /// The shared memory segment version is not supported.
    SegmentVersionNotSupported,
}

impl fmt::Display for ShmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ShmError::SyscallError(errno, c_str) => {
                write!(f, "Errno: {errno:?} Details: {c_str:?}")
            }
            ShmError::SegmentNotInitialized => {
                write!(f, "The shared memory segment is not initialized.")
            }
            ShmError::SegmentMalformed => {
                write!(f, "The shared memory segment is initialized but malformed.")
            }
            ShmError::CausalityBreach => {
                write!(f, "Failed causality check when comparing timestamps.")
            }
            ShmError::SegmentVersionNotSupported => {
                write!(f, "The shared memory segment version is not supported.")
            }
        }
    }
}

impl Error for ShmError {}

/// Definition of mutually exclusive clock status exposed to the reader.
///
/// Note the data layout is explicitly set to i32. This enum is a field of the ClockBound shared
/// memory segment, and its representation *may* be different for C code compiled with specific
/// flags. Making it explicit removes this risk and ambiguity.
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ClockStatus {
    /// The status of the clock is unknown.
    /// In this clock status, error-bounded timestamps should not be trusted.
    Unknown = 0,

    /// The clock is kept accurate by the synchronization daemon.
    /// In this clock status, error-bounded timestamps can be trusted.
    Synchronized = 1,

    /// The clock is free running and not updated by the synchronization daemon.
    /// In this clock status, error-bounded timestamps can be trusted.
    FreeRunning = 2,

    /// The clock has been disrupted and the accuracy of time cannot be bounded.
    /// In this clock status, error-bounded timestamps should not be trusted.
    Disrupted = 3,
}

/// Structure that holds the `ClockErrorBound` data captured at a specific point in time and valid
/// until a subsequent point in time.
///
/// The `ClockErrorBound` structure supports calculating the actual bound on clock error at any time,
/// using its `now()` method. The internal fields are not meant to be accessed directly.
///
/// Note that the timestamps in between which this `ClockErrorBound` data is valid are captured using
/// a `CLOCK_MONOTONIC_COARSE` clock. The monotonic clock id is required to correctly measure the
/// duration during which clock drift possibly accrues, and avoid events when the clock is set,
/// smeared or affected by leap seconds.
///
/// The structure is shared across the Shared Memory segment and has a C representation to enforce
/// this specific layout.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ClockErrorBoundV2 {
    /// The `CLOCK_MONOTONIC_COARSE` timestamp recorded when the bound on clock error was
    /// calculated. The current implementation relies on Chrony tracking data, which accounts for
    /// the dispersion between the last clock processing event, and the reading of tracking data.
    as_of: TimeSpec,

    /// The `CLOCK_MONOTONIC_COARSE` timestamp beyond which the bound on clock error should not be
    /// trusted. This is a useful signal that the communication with the synchronization daemon is
    /// has failed, for example.
    void_after: TimeSpec,

    /// An absolute upper bound on the accuracy of the `CLOCK_REALTIME` clock with regards to true
    /// time at the instant represented by `as_of`.
    bound_nsec: i64,

    /// Disruption marker.
    ///
    /// This value is incremented (by an unspecified delta) each time the clock has been disrupted.
    /// This count value is specific to a particular VM/EC2 instance.
    pub disruption_marker: u64,

    /// Maximum drift rate of the clock between updates of the synchronization daemon. The value
    /// stored in `bound_nsec` should increase by the following to account for the clock drift
    /// since `bound_nsec` was computed:
    /// `bound_nsec += max_drift_ppb * (now - as_of)`
    max_drift_ppb: u32,

    /// The synchronization daemon status indicates whether the daemon is synchronized,
    /// free-running, etc.
    clock_status: ClockStatus,

    /// Clock disruption support enabled flag.
    ///
    /// This indicates whether or not the ClockBound daemon was started with a
    /// configuration that supports detecting clock disruptions.
    pub clock_disruption_support_enabled: bool,

    /// Padding.
    _padding: [u8; 7],
}

impl ClockErrorBoundV2 {
    /// Create a new `ClockErrorBound` struct.
    pub fn new(
        as_of: TimeSpec,
        void_after: TimeSpec,
        bound_nsec: i64,
        disruption_marker: u64,
        max_drift_ppb: u32,
        clock_status: ClockStatus,
        clock_disruption_support_enabled: bool,
    ) -> ClockErrorBoundV2 {
        ClockErrorBoundV2 {
            as_of,
            void_after,
            bound_nsec,
            disruption_marker,
            max_drift_ppb,
            clock_status,
            clock_disruption_support_enabled,
            _padding: [0u8; 7],
        }
    }

    /// The `ClockErrorBoundV2` implementation of `now()`, a `clock_gettime()` equivalent but with
    /// bound on clock accuracy.
    ///
    /// Returns a pair of (earliest, latest) timespec between which current time exists. The
    /// interval width is twice the clock error bound (ceb) such that:
    ///   (earliest, latest) = ((now - ceb), (now + ceb))
    /// The function also returns a clock status to assert that the clock is being synchronized, or
    /// free-running, or ...
    #[expect(clippy::missing_errors_doc, reason = "todo")]
    pub fn now(&self) -> Result<ClockBoundNowResult, ShmError> {
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
    #[expect(
        clippy::cast_precision_loss,
        clippy::cast_possible_truncation,
        reason = "todo, come back and evaluate impact"
    )]
    fn compute_bound_at(
        &self,
        real: TimeSpec,
        mono: TimeSpec,
    ) -> Result<ClockBoundNowResult, ShmError> {
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
            // If the status in the shared memory segment is Unknown or Disrupted, returns that
            // status.
            ClockStatus::Unknown | ClockStatus::Disrupted => self.clock_status,

            // If the status is Synchronized or FreeRunning, the expectation from the client is
            // that the data is useable. However, if the clockbound daemon died or has not update
            // the shared memory segment in a while, the status written to the shared memory
            // segment may not be reliable anymore.
            ClockStatus::Synchronized | ClockStatus::FreeRunning => {
                if mono < self.as_of + CLOCKBOUND_RESTART_GRACE_PERIOD {
                    // Allow for a restart of the daemon, for a short period of time, the status is
                    // trusted to be correct.
                    self.clock_status
                } else if mono < self.void_after {
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
        let causality_blur = self.as_of - TimeSpec::new(0, 1000);

        let duration = if mono >= self.as_of {
            // Happy path, no causality doubt
            mono - self.as_of
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
            self.bound_nsec + (duration_sec * f64::from(self.max_drift_ppb)) as i64,
        );

        // Build the (earliest, latest) interval within which true time exists.
        let earliest = real - updated_bound;
        let latest = real + updated_bound;

        Ok(ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        })
    }
}

impl ClockBoundSnapshot for ClockErrorBoundV2 {
    /// The `ClockErrorBoundV2` implementation of `now()`.
    ///
    /// This version relies on the system clock to retrieve the current time as well as grow the
    /// bound on the clock error at a constant rate.
    fn now(&self) -> Result<ClockBoundNowResult, ShmError> {
        // Read the clock, start with the REALTIME one to be as close as possible to the event the
        // caller is interested in. The monotonic clock should be read after. It is correct for the
        // process be preempted between the two calls: a delayed read of the monotonic clock will
        // make the bound on clock error more pessimistic, but remains correct.
        let real = clock_gettime_safe(CLOCK_REALTIME)?;
        let mono = clock_gettime_safe(CLOCK_MONOTONIC)?;

        self.compute_bound_at(real, mono)
    }
}

#[cfg(test)]
mod t_lib {
    use super::*;

    // Convenience macro to build ClockBoundError for unit tests
    macro_rules! clockbound_v2 {
        (($asof_tv_sec:literal, $asof_tv_nsec:literal), ($after_tv_sec:literal, $after_tv_nsec:literal)) => {
            ClockErrorBoundV2::new(
                TimeSpec::new($asof_tv_sec, $asof_tv_nsec), // as_of
                TimeSpec::new($after_tv_sec, $after_tv_nsec), // void_after
                10000,                                      // bound_nsec
                0,                                          // disruption_marker
                1000,                                       // max_drift_ppb
                ClockStatus::Synchronized,                  // clock_status
                true,                                       // clock_disruption_support_enabled
            )
        };
    }

    /// Assert the bound on clock error is computed correctly
    #[test]
    fn compute_bound_ok() {
        let ceb = clockbound_v2!((0, 0), (10, 0));
        let real = TimeSpec::new(2, 0);
        let mono = TimeSpec::new(2, 0);

        let ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        } = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 2 seconds have passed since the bound was snapshot, hence 2 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec(), 1);
        assert_eq!(earliest.tv_nsec(), 1_000_000_000 - 12_000);
        assert_eq!(latest.tv_sec(), 2);
        assert_eq!(latest.tv_nsec(), 12_000);
        assert_eq!(clock_status, ClockStatus::Synchronized);
    }

    /// Assert the bound on clock error is computed correctly, with realtime and monotonic clocks
    /// disagreeing on time
    #[test]
    fn compute_bound_ok_when_real_ahead() {
        let ceb = clockbound_v2!((0, 0), (10, 0));
        let real = TimeSpec::new(20, 0); // realtime clock way ahead
        let mono = TimeSpec::new(4, 0);

        let ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        } = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 4 seconds have passed since the bound was snapshot, hence 4 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec(), 19);
        assert_eq!(earliest.tv_nsec(), 1_000_000_000 - 14_000);
        assert_eq!(latest.tv_sec(), 20);
        assert_eq!(latest.tv_nsec(), 14_000);
        assert_eq!(clock_status, ClockStatus::Synchronized);
    }

    /// Assert the clock status is FreeRunning if the ClockErrorBound data is passed the grace
    /// period
    #[test]
    fn compute_bound_force_free_running_status() {
        let ceb = clockbound_v2!((0, 0), (100, 0));
        let real = TimeSpec::new(8, 0);
        let mono = TimeSpec::new(8, 0);

        let ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        } = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 8 seconds have passed since the bound was snapshot, hence 8 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec(), 7);
        assert_eq!(earliest.tv_nsec(), 1_000_000_000 - 18_000);
        assert_eq!(latest.tv_sec(), 8);
        assert_eq!(latest.tv_nsec(), 18_000);
        assert_eq!(clock_status, ClockStatus::FreeRunning);
    }

    /// Assert the clock status is Unknown if the ClockErrorBound data is passed void_after
    #[test]
    fn compute_bound_unknown_status_if_expired() {
        let ceb = clockbound_v2!((0, 0), (5, 0));
        let real = TimeSpec::new(10, 0);
        let mono = TimeSpec::new(10, 0); // Passed void_after

        let ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        } = ceb
            .compute_bound_at(real, mono)
            .expect("Failed to compute bound");

        // 10 seconds have passed since the bound was snapshot, hence 10 microsec of drift on top of
        // the default 10 microsec put in the ClockBoundError data
        assert_eq!(earliest.tv_sec(), 9);
        assert_eq!(earliest.tv_nsec(), 1_000_000_000 - 20_000);
        assert_eq!(latest.tv_sec(), 10);
        assert_eq!(latest.tv_nsec(), 20_000);
        assert_eq!(clock_status, ClockStatus::Unknown);
    }

    /// Assert errors are returned if the ClockBoundError data is malformed with bad drift
    #[test]
    fn compute_bound_bad_drift() {
        let mut ceb = clockbound_v2!((0, 0), (10, 0));
        let real = TimeSpec::new(5, 0);
        let mono = TimeSpec::new(5, 0);
        ceb.max_drift_ppb = 2_000_000_000;

        assert!(ceb.compute_bound_at(real, mono).is_err());
    }

    /// Assert errors are returned if the ClockBoundError data snapshot has been taken after
    /// reading clocks at 'now'
    #[test]
    fn compute_bound_causality_break() {
        let ceb = clockbound_v2!((5, 0), (10, 0));
        let real = TimeSpec::new(1, 0);
        let mono = TimeSpec::new(1, 0);

        let res = ceb.compute_bound_at(real, mono);

        assert!(res.is_err());
    }
}
