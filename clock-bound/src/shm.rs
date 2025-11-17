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
mod tsc;
mod writer;

// Re-exports reader and writer. The writer is conditionally included under the "writer" feature.
use common::{CLOCK_MONOTONIC, CLOCK_REALTIME, clock_gettime_safe};
pub use reader::ShmReader;
use tsc::read_timestamp_counter_begin;
pub use writer::{ShmWrite, ShmWriter};

use bon::Builder;
use errno::Errno;
use nix::sys::time::{TimeSpec, TimeValLike};
use std::error::Error;
use std::ffi::CStr;
use std::fmt;

pub const CLOCKBOUND_SHM_DEFAULT_PATH: &str = "/var/run/clockbound/shm0";

const CLOCKBOUND_RESTART_GRACE_PERIOD: TimeSpec = TimeSpec::new(5, 0);
const NANOS_PER_SECOND: f64 = 1_000_000_000.0;

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
    V3(ClockErrorBoundV3),
}

impl ClockErrorBound {
    pub fn as_of(&self) -> TimeSpec {
        match self {
            ClockErrorBound::V2(ceb) => ceb.as_of,
            ClockErrorBound::V3(ceb) => ceb.as_of,
        }
    }

    pub fn void_after(&self) -> TimeSpec {
        match self {
            ClockErrorBound::V2(ceb) => ceb.void_after,
            ClockErrorBound::V3(ceb) => ceb.void_after,
        }
    }

    pub fn bound_nsec(&self) -> i64 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.bound_nsec,
            ClockErrorBound::V3(ceb) => ceb.bound_nsec,
        }
    }

    pub fn max_drift_ppb(&self) -> u32 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.max_drift_ppb,
            ClockErrorBound::V3(ceb) => ceb.max_drift_ppb,
        }
    }

    pub fn clock_status(&self) -> ClockStatus {
        match self {
            ClockErrorBound::V2(ceb) => ceb.clock_status,
            ClockErrorBound::V3(ceb) => ceb.clock_status,
        }
    }

    pub fn disruption_marker(&self) -> u64 {
        match self {
            ClockErrorBound::V2(ceb) => ceb.disruption_marker,
            ClockErrorBound::V3(ceb) => ceb.disruption_marker,
        }
    }

    pub fn clock_disruption_support_enabled(&self) -> bool {
        match self {
            ClockErrorBound::V2(ceb) => ceb.clock_disruption_support_enabled,
            ClockErrorBound::V3(ceb) => ceb.clock_disruption_support_enabled,
        }
    }
}

impl ClockBoundSnapshot for ClockErrorBound {
    fn now(&self) -> Result<ClockBoundNowResult, ShmError> {
        match self {
            ClockErrorBound::V2(ceb) => ceb.now(),
            ClockErrorBound::V3(ceb) => ceb.now(),
        }
    }
}

/// Generic `ClockErrorBound` builder.
#[derive(Builder)]
// Rename auto-generated build() function into build_internal so we have a custom finishing
// function to create the enum variants
#[builder(finish_fn(vis = "", name = build_internal))]
pub struct ClockErrorBoundGeneric {
    #[builder(default)]
    as_of_tsc: u64,

    #[builder(default = TimeSpec::new(0, 0))]
    as_of: TimeSpec,

    #[builder(default = TimeSpec::new(0, 0))]
    void_after: TimeSpec,

    #[builder(default)]
    bound_nsec: i64,

    #[builder(default)]
    period: f64,

    #[builder(default)]
    period_err: f64,

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
    pub fn build(self, layout_version: ClockErrorBoundLayoutVersion) -> ClockErrorBound {
        // Build the ClockErrorBoundGeneric object
        let ceb = self.build_internal();

        // Build the specific version of the ClockErrorBound
        match layout_version {
            ClockErrorBoundLayoutVersion::V2 => ClockErrorBound::V2(ClockErrorBoundV2::new(
                ceb.as_of,
                ceb.void_after,
                ceb.bound_nsec,
                ceb.disruption_marker,
                ceb.max_drift_ppb,
                ceb.clock_status,
                ceb.clock_disruption_support_enabled,
            )),
            ClockErrorBoundLayoutVersion::V3 => ClockErrorBound::V3(ClockErrorBoundV3::new(
                ceb.as_of_tsc,
                ceb.as_of,
                ceb.void_after,
                ceb.period,
                ceb.period_err,
                ceb.bound_nsec,
                ceb.disruption_marker,
                ceb.max_drift_ppb,
                ceb.clock_status,
                ceb.clock_disruption_support_enabled,
            )),
        }
    }
}

#[derive(Copy, Clone)]
pub enum ClockErrorBoundLayoutVersion {
    V2,
    V3,
}

impl TryFrom<u8> for ClockErrorBoundLayoutVersion {
    type Error = ShmError;
    fn try_from(value: u8) -> Result<Self, ShmError> {
        match value {
            2 => Ok(ClockErrorBoundLayoutVersion::V2),
            3 => Ok(ClockErrorBoundLayoutVersion::V3),
            _ => Err(ShmError::SegmentVersionNotSupported),
        }
    }
}

impl TryFrom<u16> for ClockErrorBoundLayoutVersion {
    type Error = ShmError;
    fn try_from(value: u16) -> Result<Self, ShmError> {
        match value {
            2 => Ok(ClockErrorBoundLayoutVersion::V2),
            3 => Ok(ClockErrorBoundLayoutVersion::V3),
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
    disruption_marker: u64,

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
    clock_disruption_support_enabled: bool,

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

/// Structure that holds the `ClockErrorBound` data captured at a specific point in time and valid
/// until a subsequent point in time.
///
/// The `ClockErrorBound` structure supports calculating the actual bound on clock error at any time,
/// using its `now()` method. The internal fields are not meant to be accessed directly.
///
/// Note that this version of the layout allow to not use the OS system clock to retrieve the
/// current time or grow the clock error bound.
///
/// The structure is shared across the Shared Memory segment and has a C representation to enforce
/// this specific layout.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct ClockErrorBoundV3 {
    /// TSC counter value identifying this clock update.
    ///
    /// The TSC counter timestamp marking the time the clock and the clock error bound where
    /// updated last. It represents the same instant as `as_of`.
    as_of_tsc: u64,

    /// Timestamp of this clock update.
    ///
    /// The nanosecond resolution timestamp marking the time the clock and the clock error bound
    /// where updated last. This timestamp is derived from `as_of_tsc`.
    as_of: TimeSpec,

    /// Time after which this clock update is void.
    ///
    /// The nanosecond timestamp beyond which the bound on clock error should not be trusted. This
    /// is a useful signal that the communication with the synchronization daemon is has failed,
    /// for example.
    void_after: TimeSpec,

    /// Oscillator period estimate.
    ///
    /// The period of the oscillator, represented as a fractional part of a second.
    period_frac: u64,

    /// Oscillator period estimate error.
    ///
    /// The error on the estimate of the period of the oscillator, in ppb, represented as a
    /// fractional part of a second.
    period_err_frac: u64,

    /// Clock Error Bound
    ///
    /// An absolute upper bound on the accuracy of the feed-forward synchronization clock with
    /// regards to true time at the instant represented by `as_of` and `as_of_tsc`.
    bound_nsec: i64,

    /// Disruption marker.
    ///
    /// This value is incremented (by an unspecified delta) each time the clock has been disrupted.
    /// This count value is specific to a particular VM/EC2 instance.
    disruption_marker: u64,

    /// Maximum drift rate in part-per-billion.
    ///
    /// Maximum drift rate of the clock between updates of the synchronization daemon. The value
    /// stored in `bound_nsec` should increase by the following to account for the clock drift
    /// since `bound_nsec` was computed:
    /// `bound_nsec += max_drift_ppb * (now - as_of)`
    max_drift_ppb: u32,

    /// Clock status.
    ///
    /// The synchronization daemon status indicates whether the daemon is synchronized,
    /// free-running, etc.
    clock_status: ClockStatus,

    /// Clock disruption support enabled flag.
    ///
    /// This indicates whether or not the ClockBound daemon was started with a
    /// configuration that supports detecting clock disruptions.
    clock_disruption_support_enabled: bool,

    /// Period shift
    ///
    /// This is a scaling parameter to convert the `period` into a fractional representation with
    /// significant digits.
    period_shift: u8,

    /// Period error shift
    ///
    /// This is a scaling parameter to convert the `period_err` into a fractional representation with
    /// significant digits.
    period_err_shift: u8,

    /// Padding.
    _padding: [u8; 5],
}

impl ClockErrorBoundV3 {
    /// Create a new `ClockErrorBound` struct.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        as_of_tsc: u64,
        as_of: TimeSpec,
        void_after: TimeSpec,
        period: f64,
        period_err: f64,
        bound_nsec: i64,
        disruption_marker: u64,
        max_drift_ppb: u32,
        clock_status: ClockStatus,
        clock_disruption_support_enabled: bool,
    ) -> ClockErrorBoundV3 {
        // Convert period and period_err into u64 representation
        let p_frac = PeriodFrac::from(period);
        let p_err_frac = PeriodFrac::from(period_err);

        ClockErrorBoundV3 {
            as_of_tsc,
            as_of,
            void_after,
            period_frac: p_frac.frac,
            period_err_frac: p_err_frac.frac,
            bound_nsec,
            disruption_marker,
            max_drift_ppb,
            clock_status,
            clock_disruption_support_enabled,
            period_shift: p_frac.shift,
            period_err_shift: p_err_frac.shift,
            _padding: [0u8; 5],
        }
    }

    /// Get the oscillator period as a floating point value in seconds.
    fn period(&self) -> f64 {
        f64::from(PeriodFrac {
            frac: self.period_frac,
            shift: self.period_shift,
        })
    }

    /// Get the oscillator period error as a floating point value.
    fn period_err(&self) -> f64 {
        f64::from(PeriodFrac {
            frac: self.period_err_frac,
            shift: self.period_err_shift,
        })
    }

    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_possible_truncation)]
    fn compute_bound_at_tsc(&self, now_tsc: u64) -> Result<ClockBoundNowResult, ShmError> {
        // Sanity checks:
        // - `now()` should operate on a consistent snapshot of the shared memory segment, and
        //   causality between mono and as_of should be enforced.
        // - a extremely high value of the `max_drift_ppb` is a sign of something going wrong
        if self.max_drift_ppb >= 1_000_000_000 {
            return Err(ShmError::SegmentMalformed);
        }

        // Compute the number of TSC cycles between now and the instant the ff-sync clock was
        // updated last. This is computed of a TSC value stored in the snapshot, hence this
        // duration should never be negative.
        let duration_tsc = now_tsc.saturating_sub(self.as_of_tsc);
        let duration_nsec = duration_tsc as f64 * self.period() * NANOS_PER_SECOND;

        // Convert the TSC timestamp into seconds with a linear projection.
        let now = TimeSpec::nanoseconds(
            ((self.as_of.tv_nsec() as f64)
                + (NANOS_PER_SECOND * self.as_of.tv_sec() as f64)
                + duration_nsec) as i64,
        );

        // Similarly, need to grow the bound on the clock error since the last update.
        // This takes into accounts the fact that the ff-sync period is an estimate (polluted by
        // measurement noise), and that the underlying oscillator drifts (possibly at a worse
        // possible rate) in between consecutive clock adjustments.
        let oscillator_err_nsec = duration_nsec * f64::from(self.max_drift_ppb) / NANOS_PER_SECOND;
        let p_estimate_err_nsec = duration_nsec * self.period_err() / NANOS_PER_SECOND;

        let updated_bound = TimeSpec::nanoseconds(
            (self.bound_nsec as f64 + oscillator_err_nsec + p_estimate_err_nsec) as i64,
        );

        // Build the (earliest, latest) interval within which true time exists.
        let earliest = now - updated_bound;
        let latest = now + updated_bound;

        // If the ClockErrorBound data has not been updated "recently", the status of the clock
        // cannot be guaranteed. Things are ambiguous, the synchronization daemon may be dead, or
        // its interaction with the clockbound daemon is broken, or ... In any case, we signal the
        // caller that guarantees are gone. We could return an Err here, but choosing to leverage
        // ClockStatus instead, and putting the responsibility on the caller to check the clock
        // status value being returned.
        let duration = TimeSpec::nanoseconds(duration_nsec as i64);
        let clock_status = match self.clock_status {
            // If the status in the shared memory segment is Unknown or Disrupted, returns that
            // status.
            ClockStatus::Unknown | ClockStatus::Disrupted => self.clock_status,

            // If the status is Synchronized or FreeRunning, the expectation from the client is
            // that the data is useable. However, if the clockbound daemon died or has not update
            // the shared memory segment in a while, the status written to the shared memory
            // segment may not be reliable anymore.
            ClockStatus::Synchronized | ClockStatus::FreeRunning => {
                if duration < CLOCKBOUND_RESTART_GRACE_PERIOD {
                    // Allow for a restart of the daemon, for a short period of time, the status is
                    // trusted to be correct.
                    self.clock_status
                } else if now < self.void_after {
                    // Beyond the grace period, for a free running status.
                    ClockStatus::FreeRunning
                } else {
                    // If beyond void_after, no guarantee is provided anymore.
                    ClockStatus::Unknown
                }
            }
        };

        Ok(ClockBoundNowResult {
            earliest,
            latest,
            clock_status,
        })
    }
}

impl ClockBoundSnapshot for ClockErrorBoundV3 {
    /// The `ClockErrorBoundV3` implementation of `now()`.
    ///
    /// This version relies on the system clock to retrieve the current time as well as grow the
    /// bound on the clock error at a constant rate.
    fn now(&self) -> Result<ClockBoundNowResult, ShmError> {
        let now_tsc = read_timestamp_counter_begin();
        self.compute_bound_at_tsc(now_tsc)
    }
}

struct PeriodFrac {
    frac: u64,
    shift: u8,
}

impl PeriodFrac {
    /// Calculate the multiplication factor to maximize the number of significant digits when
    /// converting the period from a floating point to an integer representation.
    ///
    /// # Panic:
    /// Panic if the period passed is larger that 1 second.
    ///
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    fn calculate_frac_shift(period: f64) -> u8 {
        // 1HZ and slower should not be seen.
        assert!(
            period < 1.0,
            "Cannot convert period larger than 1 second: {period}"
        );

        // Protects against the case where a zero period is passed in.
        if period == 0_f64 {
            return 0_u8;
        }
        let freq: u64 = (1.0 / period) as u64;
        // Cast: at most 64 zeros in a u64, hence can never go over u8::MAX.
        (64 - freq.leading_zeros() - 1) as u8
    }
}

impl From<f64> for PeriodFrac {
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_possible_truncation)]
    #[allow(clippy::cast_sign_loss)]
    fn from(value: f64) -> Self {
        let shift = PeriodFrac::calculate_frac_shift(value);
        // Cast: 64 + 255 unsigned does fit into a i32 without risk of sign error
        let scale = 64 + i32::from(shift);
        let frac = (value * 2_f64.powi(scale)) as u64;
        PeriodFrac { frac, shift }
    }
}

impl From<PeriodFrac> for f64 {
    #[allow(clippy::cast_precision_loss)]
    #[allow(clippy::cast_possible_truncation)]
    fn from(value: PeriodFrac) -> Self {
        let denominator = 2_f64.powi(64 + i32::from(value.shift));
        (value.frac as f64) / denominator
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

    #[test]
    fn test_ceb_v3_new() {
        let ceb = ClockErrorBoundV3::new(
            1000,                      // as_of_tsc
            TimeSpec::new(1, 0),       // as_of
            TimeSpec::new(10, 0),      // void_after
            1e-9,                      // period
            1e-12,                     // period_err
            5000,                      // bound_nsec
            42,                        // disruption_marker
            1000,                      // max_drift_ppb
            ClockStatus::Synchronized, // clock_status
            true,                      // clock_disruption_support_enabled
        );

        assert_eq!(ceb.as_of_tsc, 1000);
        assert_eq!(ceb.as_of, TimeSpec::new(1, 0));
        assert_eq!(ceb.void_after, TimeSpec::new(10, 0));
        assert_eq!(ceb.bound_nsec, 5000);
        assert_eq!(ceb.disruption_marker, 42);
        assert_eq!(ceb.max_drift_ppb, 1000);
        assert_eq!(ceb.clock_status, ClockStatus::Synchronized);
        assert_eq!(ceb.clock_disruption_support_enabled, true);

        // Test period conversion
        let period = ceb.period();
        assert!((period - 1e-9).abs() < 1e-15);

        let period_err = ceb.period_err();
        assert!((period_err - 1e-12).abs() < 1e-18);
    }

    #[test]
    fn test_ceb_v3_period_conversion() {
        let ceb = ClockErrorBoundV3::new(
            0,
            TimeSpec::new(0, 0),
            TimeSpec::new(10, 0),
            2.5e-9, // 400 MHz
            1e-11,
            1000,
            0,
            1000,
            ClockStatus::Synchronized,
            true,
        );

        let period = ceb.period();
        let relative_error = (period - 2.5e-9).abs() / 2.5e-9;
        assert!(relative_error < 1e-10);

        let period_err = ceb.period_err();
        let relative_error = (period_err - 1e-11).abs() / 1e-11;
        assert!(relative_error < 1e-10);
    }

    #[test]
    fn test_v3_compute_bound_at_tsc_synchronized_status() {
        // Create a V3 CEB with known values
        let ceb = ClockErrorBoundV3::new(
            1_000_000_000,         // as_of_tsc (1 billion cycles)
            TimeSpec::new(1, 0),   // as_of = 1 second
            TimeSpec::new(100, 0), // void_after = 100 seconds
            1e-9,                  // period = 1 ns (1 GHz clock)
            1e-12,                 // period_err = 1ps
            10_000,                // bound_nsec = 10 microseconds
            0,                     // disruption_marker
            1000,                  // max_drift_ppb = 1 ppm
            ClockStatus::Synchronized,
            true,
        );

        // Simulate reading TSC 2 seconds later (2 billion more cycles at 1 GHz)
        let now_tsc = 3_500_000_000;

        let result = ceb.compute_bound_at_tsc(now_tsc).expect("Should succeed");

        // Expected time: as_of + 2 seconds = 3 seconds
        assert_eq!(result.earliest.tv_sec(), 3); // approximately
        assert_eq!(result.latest.tv_sec(), 3); // approximately

        // Status should still be Synchronized (within grace period)
        assert_eq!(result.clock_status, ClockStatus::Synchronized);
    }

    // Assert that typical TSC periods (1Hz to 10 GHz range) are converted into scaled integers
    // without a loss of precision.
    #[test]
    fn test_period_frac_conversion_typical_periods() {
        let periods = [1e-3, 1e-6, 1e-7, 1e-8, 1e-9, 2e-9, 5e-9, 1e-10];

        for &period in &periods {
            let frac = PeriodFrac::from(period);
            let result: f64 = f64::from(frac);
            assert!(result == period);
        }
    }

    // Assert atypical TSC periods are converted into scaled integers
    // with a minimum loss of precision.
    #[test]
    fn test_period_frac_conversion_edge_cases() {
        // Very small period (very high frequency)
        let small_period = 1e-25;
        let frac = PeriodFrac::from(small_period);
        let result: f64 = f64::from(frac);
        let relative_error = (result - small_period).abs() / small_period;
        assert!(relative_error < 1e-10);

        // Larger period (lower frequency)
        let large_period = 0.1;
        let frac = PeriodFrac::from(large_period);
        let result: f64 = f64::from(frac);
        let relative_error = (result - large_period).abs() / large_period;
        assert!(relative_error < 1e-10);
    }

    // Assert that the conversion panics on non-realistic frequencies.
    #[test]
    #[should_panic(expected = "Cannot convert period larger than 1 second")]
    fn test_period_frac_conversion_panci() {
        let large_period = 1.0;
        let _ = PeriodFrac::from(large_period);
    }

    #[test]
    fn test_calculate_frac_shift_typical() {
        // For a 1 GHz clock (period = 1e-9), frequency = 1e9
        // 1e9 in binary is about 30 bits, so shift should be around 29
        let period = 1e-9;
        let shift = PeriodFrac::calculate_frac_shift(period);
        assert!(shift >= 29 && shift <= 30, "shift = {}", shift);

        // For a 2.5 GHz clock (period = 4e-10), frequency = 2.5e9
        // 2.5e9 in binary is about 31 bits
        let period = 4e-10;
        let shift = PeriodFrac::calculate_frac_shift(period);
        assert!(shift >= 30 && shift <= 32, "shift = {}", shift);
    }

    #[test]
    fn test_calculate_frac_shift_zero() {
        // Zero period should return 0 (max of 0 and negative value)
        let period = 0.0;
        let shift = PeriodFrac::calculate_frac_shift(period);
        assert_eq!(shift, 0);
    }

    #[test]
    fn test_zero_period_frac_conversion() {
        // Test that zero period doesn't panic and gives reasonable result
        let period = 0.0;
        let frac = PeriodFrac::from(period);
        assert_eq!(frac.shift, 0);
        assert_eq!(frac.frac, 0);

        let result: f64 = f64::from(frac);
        assert_eq!(result, 0.0);
    }

    #[test]
    fn test_precision_maintained() {
        // Test that we maintain good precision across conversions
        let period = 2.718281828e-9; // Some arbitrary value
        let frac = PeriodFrac::from(period);
        let result: f64 = f64::from(frac);

        // Should maintain at least 10 significant digits
        let relative_error = (result - period).abs() / period;
        assert!(relative_error < 1e-10);
    }

    #[test]
    fn test_frac_representation_property() {
        // Test that the fixed-point representation makes sense
        let period = 1e-9;
        let frac = PeriodFrac::from(period);

        // frac should be non-zero for non-zero period
        assert!(frac.frac > 0);

        // shift should be reasonable (not 0 or 255)
        assert!(frac.shift > 0 && frac.shift < 64);
    }
}
