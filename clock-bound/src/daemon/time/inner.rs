//! Time representation
//!
//! Simpler representation of time than `nix::time::TimeSpec`. Just abstractions over integer math

#[cfg(feature = "time-string-parse")]
pub mod string_parse;

use std::{
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
};

#[cfg(target_os = "linux")]
use libc::timeval;
use nix::sys::time::TimeSpec;
use serde::{Deserialize, Serialize};

/// Abstraction used to reuse basic time arithmetic, but allow for different types based on its usage
pub trait Type {}

/// Abstraction for time type whose tick unit is approximately one nanosecond
pub trait NanoType: Type {
    /// The type prefix to use in the `Debug` impl
    const INSTANT_PREFIX: &'static str;
    const DURATION_PREFIX: &'static str;
}

/// An offset and round-trip-time associated with a comparison of two clocks.
///
/// Generally, the "offset" of two clocks can be estimated via use of interleaved reads,
/// e.g. reads: `our_clock_t1`, `other_clock_t2`, `our_clock_t3`
/// and the offset would be approximately the difference between the midpoint of the `our_clock_t1` and `our_clock_t3`
/// reads and the `other_clock_t2` reads.
/// The comparison is bounded by the round-trip-time of this measurement, and thus is useful for
/// determining the quality of the sample or bounding the clock error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ClockOffsetAndRtt<T: NanoType + Copy> {
    /// Offset
    offset: Diff<T>,
    /// RTT
    rtt: Diff<T>,
}

impl<T: NanoType + Copy> ClockOffsetAndRtt<T> {
    pub(crate) fn new(offset: Diff<T>, rtt: Diff<T>) -> Self {
        Self { offset, rtt }
    }

    pub fn offset(&self) -> Diff<T> {
        self.offset
    }

    pub fn rtt(&self) -> Diff<T> {
        self.rtt
    }
}

#[cfg_attr(test, mockall::automock)]
pub trait Clock<T: Type + Copy> {
    /// Read the current clock time.
    fn get_time(&self) -> Time<T>;
}

/// Extension trait for Clock that provides offset and RTT measurement functionality
pub trait ClockExt<T: NanoType + Copy>: Clock<T> {
    /// Get the offset and RTT measurement for this reading between this clock and another clock.
    /// The "offset" is from the caller to the `other` clock - e.g. if `self` is running behind `other`,
    /// we would expect a negative value, and if it `self` running ahead of `other`, we expect a positive value.
    fn get_offset_and_rtt(&self, other: &impl Clock<T>) -> ClockOffsetAndRtt<T> {
        let our_read1 = self.get_time();
        let their_read = other.get_time();
        let our_read2 = self.get_time();
        let mid = our_read1.midpoint(our_read2);
        let offset = mid - their_read;
        let rtt = our_read2 - our_read1;
        ClockOffsetAndRtt::new(offset, rtt)
    }
}

/// Blanket implementation of `ClockExt` for all types that implement Clock
impl<T: NanoType + Copy, C: Clock<T>> ClockExt<T> for C {}

/// Abstract type for Time while keeping arithmetic consistent
///
/// This type is not usually used directly, but rather through the [`Instant`](super::Instant) and [`Tsc`](super::TscCount) types.
#[derive(
    Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Time<T> {
    instant: i64,
    _marker: std::marker::PhantomData<T>,
}

impl<T: NanoType + Copy> std::fmt::Debug for Time<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        debug_nanos(f, T::INSTANT_PREFIX, self.as_nanos())
    }
}

impl<T> Time<T> {
    /// Get the inner value
    pub const fn get(&self) -> i64 {
        self.instant
    }
}

impl<T> From<Time<T>> for i64 {
    fn from(value: Time<T>) -> Self {
        value.instant
    }
}

// type guarded to be sealed construction
impl<T: Type> Time<T> {
    /// Zero valued epoch
    pub const EPOCH: Self = Self::new(0);

    /// constructor
    pub const fn new(instant: i64) -> Self {
        Self {
            instant,
            _marker: PhantomData,
        }
    }
}

impl<T: Type + Copy> Time<T> {
    /// Get the midpoint between 2 instants
    #[must_use]
    pub const fn midpoint(&self, other: Self) -> Self {
        Self::new(self.instant.midpoint(other.instant))
    }
}

impl<T: Type> From<i64> for Time<T> {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

impl<T: Type> Sub for Time<T> {
    type Output = Diff<T>;

    fn sub(self, rhs: Self) -> Self::Output {
        Self::Output::new(self.instant - rhs.instant)
    }
}

impl<T: Type> Add<Diff<T>> for Time<T> {
    type Output = Time<T>;

    fn add(self, rhs: Diff<T>) -> Self::Output {
        Self::new(self.instant + rhs.duration)
    }
}

impl<T: Type> Add<Time<T>> for Diff<T> {
    type Output = Time<T>;

    fn add(self, rhs: Time<T>) -> Self::Output {
        rhs + self
    }
}

impl<T: Type> AddAssign<Diff<T>> for Time<T> {
    fn add_assign(&mut self, rhs: Diff<T>) {
        self.instant += rhs.duration;
    }
}

impl<T: Type> Sub<Diff<T>> for Time<T> {
    type Output = Self;

    fn sub(self, rhs: Diff<T>) -> Self::Output {
        Self::new(self.instant - rhs.duration)
    }
}

impl<T: Type> SubAssign<Diff<T>> for Time<T> {
    fn sub_assign(&mut self, rhs: Diff<T>) {
        self.instant -= rhs.duration;
    }
}

impl<T: NanoType + Copy> Time<T> {
    pub const UNIX_EPOCH: Self = Self::new(0);
    pub const MAX: Self = Self::new(i64::MAX);
    pub const MIN: Self = Self::new(i64::MIN);

    /// Create a new `Instant` from the number of seconds since the Unix Epoch
    pub const fn from_secs(secs: i64) -> Self {
        Self::new(secs * NANOS_PER_SEC)
    }

    /// Create a new `Instant` from the number of milliseconds since the Unix Epoch
    pub const fn from_millis(millis: i64) -> Self {
        Self::new(millis * NANOS_PER_MILLI)
    }

    /// Create a new `Instant` from the number of microseconds since the Unix Epoch
    pub const fn from_micros(micros: i64) -> Self {
        Self::new(micros * NANOS_PER_MICRO)
    }

    /// Create a new `Instant` from the number of nanoseconds since the Unix Epoch
    pub const fn from_nanos(nanos: i64) -> Self {
        Self::new(nanos)
    }

    /// Create a new `Instant` from the number of minutes since the Unix Epoch
    pub const fn from_minutes(minutes: i64) -> Self {
        Self::new(minutes * NANOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Create a new `Instant` from the number of hours since the Unix Epoch
    pub const fn from_hours(hours: i64) -> Self {
        Self::new(hours * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Create a new `Instant` from the number of days since the Unix Epoch
    pub const fn from_days(days: i64) -> Self {
        Self::new(days * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }

    /// Construct from the number of seconds and nanos since the Unix Epoch
    ///
    /// # Panics
    /// Panics if `nanos >= 1_000_000_000`, or value does not fit within the type
    pub fn from_time(secs: i64, nanos: u32) -> Self {
        assert!(nanos < 1_000_000_000, "nanos must be less than 1 second");
        let secs = Self::from_secs(secs);
        let nanos = Diff::<T>::from_nanos(i64::from(nanos));
        secs + nanos
    }

    /// Construct from a `nix::time::TimeSpec` since the Unix Epoch.
    /// Generally only expected to be called using `TimeSpec` retrieved via
    /// `nix::time::clock_gettime` call.
    ///
    /// # Panics
    /// Panics if `nanos >= 1_000_000_000`, or value does not fit within the type
    #[allow(
        clippy::cast_possible_truncation,
        reason = "timespec from clock_gettime tv_nsec should be between 0 and 1e9-1 so no truncation"
    )]
    #[allow(
        clippy::cast_sign_loss,
        reason = "timespec from clock_gettime tv_nsec should be between 0 and 1e9-1 so no loss of sign"
    )]
    pub fn from_timespec(timespec: TimeSpec) -> Self {
        Self::from_time(timespec.tv_sec(), timespec.tv_nsec() as u32)
    }

    /// Returns the total number of nanoseconds since the Unix Epoch
    pub const fn as_nanos(self) -> i64 {
        self.get()
    }

    /// Returns the total number of microseconds, rounded, since the Unix Epoch
    pub const fn as_micros(self) -> i64 {
        (self.get() + NANOS_PER_MICRO / 2) / NANOS_PER_MICRO
    }

    /// Returns the total number of milliseconds, rounded, since the Unix Epoch
    pub const fn as_millis(self) -> i64 {
        (self.get() + NANOS_PER_MILLI / 2) / NANOS_PER_MILLI
    }

    /// Returns the total number of seconds, rounded, since the Unix Epoch
    pub const fn as_seconds(self) -> i64 {
        (self.get() + NANOS_PER_SEC / 2) / NANOS_PER_SEC
    }

    /// Returns the total number of seconds, truncated
    pub const fn as_seconds_trunc(self) -> i64 {
        self.get() / NANOS_PER_SEC
    }

    /// Returns the total number of nanoseconds, truncated
    pub const fn as_nanos_trunc(self) -> i64 {
        self.get()
    }

    /// Returns the total number of minutes, rounded, since the Unix Epoch
    pub const fn as_minutes(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }

    /// Returns the total number of hours, rounded, since the Unix Epoch
    pub const fn as_hours(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }

    /// Returns the total number of days, rounded, since the Unix Epoch
    pub const fn as_days(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }
}

/// Difference between 2 [`Time`] values
///
/// It is not recommended to use this directly, but use the [`Duration`](super::Duration) or [`TscDiff`](super::TscDiff) types
#[derive(
    Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Diff<T> {
    duration: i64,
    _marker: PhantomData<T>,
}

impl<T: NanoType + Copy> std::fmt::Debug for Diff<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        debug_nanos(f, T::DURATION_PREFIX, self.as_nanos())
    }
}

impl<T> Diff<T> {
    /// Inner Count value
    pub const fn get(&self) -> i64 {
        self.duration
    }

    /// Returns a [`Diff`] that contains the absolute value to the given duration.
    #[must_use]
    pub fn abs(&self) -> Self {
        Self {
            duration: self.duration.abs(),
            _marker: PhantomData,
        }
    }
}

impl<T> From<Diff<T>> for i64 {
    fn from(value: Diff<T>) -> Self {
        value.duration
    }
}

impl<T: Type> Diff<T> {
    /// constructor
    pub const fn new(duration: i64) -> Self {
        Self {
            duration,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> From<i64> for Diff<T> {
    fn from(value: i64) -> Self {
        Self::new(value)
    }
}

impl<T: Type> Add for Diff<T> {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self {
            duration: self.duration + rhs.duration,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> Sub for Diff<T> {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self {
            duration: self.duration - rhs.duration,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> AddAssign for Diff<T> {
    fn add_assign(&mut self, rhs: Self) {
        self.duration += rhs.duration;
    }
}

impl<T: Type> SubAssign for Diff<T> {
    fn sub_assign(&mut self, rhs: Self) {
        self.duration -= rhs.duration;
    }
}

impl<T: Type> Div<usize> for Diff<T> {
    type Output = Self;

    #[allow(clippy::cast_possible_wrap)]
    fn div(self, rhs: usize) -> Self::Output {
        Self {
            duration: self.duration / rhs as i64,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> DivAssign<usize> for Diff<T> {
    #[allow(clippy::cast_possible_wrap)]
    fn div_assign(&mut self, rhs: usize) {
        self.duration /= rhs as i64;
    }
}

impl<T: Type> Mul<usize> for Diff<T> {
    type Output = Self;

    #[allow(clippy::cast_possible_wrap)]
    fn mul(self, rhs: usize) -> Self::Output {
        Self {
            duration: self.duration * rhs as i64,

            _marker: PhantomData,
        }
    }
}

impl<T: Type> Mul<Diff<T>> for usize {
    type Output = Diff<T>;

    fn mul(self, rhs: Diff<T>) -> Self::Output {
        rhs * self
    }
}

impl<T: Type> MulAssign<usize> for Diff<T> {
    #[allow(clippy::cast_possible_wrap)]
    fn mul_assign(&mut self, rhs: usize) {
        self.duration *= rhs as i64;
    }
}

impl<T: Type> Neg for Diff<T> {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self {
            duration: -self.duration,
            _marker: PhantomData,
        }
    }
}

impl<T: NanoType + Copy> Diff<T> {
    /// zero-valued diff
    pub const ZERO: Self = Self::new(0);

    /// Create a new [`Diff`] from the number of seconds
    pub const fn from_secs(secs: i64) -> Self {
        Self::new(secs * NANOS_PER_SEC)
    }

    /// Create a new [`Diff`] from the number of seconds in `f64` format
    ///
    /// Will round to the nearest nanosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_seconds_f64(secs: f64) -> Self {
        Self::new((secs * NANOS_PER_SEC as f64).round() as i64)
    }

    /// Create a new [`Diff`] from the number of milliseconds in `f64` format
    ///
    /// Will round to the nearest nanosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_millis_f64(millis: f64) -> Self {
        Self::new((millis * NANOS_PER_MILLI as f64).round() as i64)
    }

    /// Create a new [`Diff`] from the number of microseconds in `f64` format
    ///
    /// Will round to the nearest nanosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_micros_f64(micros: f64) -> Self {
        Self::new((micros * NANOS_PER_MICRO as f64).round() as i64)
    }

    /// Create a new [`Diff`] from the number of nanoseconds in `f64` format
    ///
    /// Will round to the nearest nanosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    pub const fn from_nanos_f64(nanos: f64) -> Self {
        Self::new(nanos.round() as i64)
    }

    /// Create a new [`Diff`] from the number of milliseconds
    pub const fn from_millis(millis: i64) -> Self {
        Self::new(millis * NANOS_PER_MILLI)
    }

    /// Create a new [`Diff`] from the number of microseconds
    pub const fn from_micros(micros: i64) -> Self {
        Self::new(micros * NANOS_PER_MICRO)
    }

    /// Create a new [`Diff`] from the number of nanoseconds
    pub const fn from_nanos(nanos: i64) -> Self {
        Self::new(nanos)
    }

    /// Create a new [`Diff`] from the number of minutes
    pub const fn from_minutes(minutes: i64) -> Self {
        Self::new(minutes * NANOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Create a new [`Diff`] from the number of hours
    pub const fn from_hours(hours: i64) -> Self {
        Self::new(hours * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Create a new [`Diff`] from the number of days
    pub const fn from_days(days: i64) -> Self {
        Self::new(days * NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }

    /// Construct from seconds and nanos
    ///
    /// # Panics
    /// Panics if `nanos >= 1_000_000_000`, or value does not fit within the type
    pub fn from_time(secs: i64, nanos: u32) -> Self {
        assert!(nanos < 1_000_000_000, "nanos must be less than 1 second");

        let secs = Self::from_secs(secs);
        let nanos = Self::from_nanos(i64::from(nanos));
        secs + nanos
    }

    /// Returns the total number of nanoseconds
    pub const fn as_nanos(self) -> i64 {
        self.get()
    }

    /// Returns the total number of microseconds, rounded
    pub const fn as_micros(self) -> i64 {
        (self.get() + NANOS_PER_MICRO / 2) / NANOS_PER_MICRO
    }

    /// Returns the total number of milliseconds, rounded
    pub const fn as_millis(self) -> i64 {
        (self.get() + NANOS_PER_MILLI / 2) / NANOS_PER_MILLI
    }

    /// Returns the total number of seconds, rounded
    pub const fn as_seconds(self) -> i64 {
        (self.get() + NANOS_PER_SEC / 2) / NANOS_PER_SEC
    }

    /// Returns the total number of seconds, truncated
    pub const fn as_seconds_trunc(self) -> i64 {
        self.get() / NANOS_PER_SEC
    }

    /// Returns the total number of nanoseconds, truncated
    pub const fn as_nanos_trunc(self) -> i64 {
        self.get()
    }

    /// Returns the total number of seconds as a f64
    #[expect(clippy::cast_precision_loss, reason = "division mitigates")]
    pub const fn as_seconds_f64(self) -> f64 {
        self.get() as f64 / NANOS_PER_SEC as f64
    }

    /// Returns the total number of minutes, rounded
    pub const fn as_minutes(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }

    /// Returns the total number of hours, rounded
    pub const fn as_hours(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }

    /// Returns the total number of days, rounded
    pub const fn as_days(self) -> i64 {
        const SCALE_FACTOR: i64 = NANOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY;
        (self.get() + SCALE_FACTOR / 2) / SCALE_FACTOR
    }

    /// Returns the `Duration` converted to a `timeval`.
    /// It's used by `adjtimex`/`ntp_adjtime` in the returned timestamp or in `ADJ_SETOFFSET`.
    ///
    /// A `timeval` is equivalent to `timespec` in usage, but with normally less resolution.
    /// However, a caller may supply `ADJ_NANO` to tell the kernel we want to supply a nanosecond value inside
    /// `tv_usec`, and so we do that to be consistent with our `clock_adjust` offset resolution.
    /// Thus, this does not return a correct `timeval` by the specifications of the fields themselves, but instead
    /// a `timeval` in the context of an `adjtimex` call with `ADJ_NANO` set.
    ///
    /// Gated to Linux: this function is specific to the Linux `adjtimex`/`ntp_adjtime` interface,
    /// and assumes `timeval.tv_usec` is `i64` (`suseconds_t = c_long` on 64-bit Linux). This does
    /// not hold on macOS (i32), 32-bit Linux (i32), or several BSDs.
    #[cfg(target_os = "linux")]
    pub fn to_timeval_nanos(self) -> timeval {
        let mut tv = timeval {
            tv_sec: self.as_seconds_trunc(),
            tv_usec: self.get() % NANOS_PER_SEC,
        };
        // Normalize the timeval, as `tv_usec` cannot be negative (so we push the "negative" place into `tv_sec`)
        if tv.tv_usec < 0 {
            tv.tv_sec -= 1;
            tv.tv_usec += 1_000_000_000_i64;
        }
        tv
    }
}

// print in <seconds>.<millis>_<micros>_<nanos> format.
fn debug_nanos(
    f: &mut std::fmt::Formatter<'_>,
    prefix: &'static str,
    nanos: i64,
) -> std::fmt::Result {
    let sign = if nanos < 0 { "-" } else { "" };
    let nanos_abs = nanos.abs();
    let secs = nanos_abs / NANOS_PER_SEC;
    let sub_nanos = nanos_abs % NANOS_PER_SEC;
    if sub_nanos == 0 {
        return f
            .debug_tuple(prefix)
            .field(&format_args!("{sign}{secs}.0"))
            .finish();
    }
    let millis = sub_nanos / 1_000_000;
    let micros = (sub_nanos / 1_000) % 1_000;
    let nanos_part = sub_nanos % 1_000;
    if nanos_part == 0 && micros == 0 {
        return f
            .debug_tuple(prefix)
            .field(&format_args!("{sign}{secs}.{millis:0>3}"))
            .finish();
    }
    if nanos_part == 0 {
        return f
            .debug_tuple(prefix)
            .field(&format_args!("{sign}{secs}.{millis:0>3}_{micros:0>3}"))
            .finish();
    }
    let formatted = format_args!("{sign}{secs}.{millis:0>3}_{micros:0>3}_{nanos_part:0>3}");
    f.debug_tuple(prefix).field(&formatted).finish()
}

pub(crate) const NANOS_PER_SEC: i64 = 1_000_000_000;
pub(crate) const NANOS_PER_MILLI: i64 = 1_000_000;
pub(crate) const NANOS_PER_MICRO: i64 = 1_000;
pub(crate) const SECS_PER_MINUTE: i64 = 60;
pub(crate) const MINS_PER_HOUR: i64 = 60;
pub(crate) const HOURS_PER_DAY: i64 = 24;

#[cfg(test)]
mod test {
    use rstest::rstest;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::daemon::time::{Duration, Instant, instant::Utc};

    #[derive(Clone, Copy)]
    struct TestType;

    impl Type for TestType {}

    type TestTimestamp = Time<TestType>;
    type TestDiff = Diff<TestType>;

    #[test]
    fn calc_raw_difference() {
        let a = TestTimestamp::new(100);
        let b = TestTimestamp::new(50);
        let c = a - b;
        assert_eq!(c.duration, 50);
    }

    #[test]
    fn add_raw_duration() {
        let a = TestDiff::new(100);
        let b = TestDiff::new(50);
        let c = a + b;
        assert_eq!(c.duration, 150);
    }

    #[test]
    fn add_raw_duration_to_timestamp() {
        let a = TestTimestamp::new(100);
        let b = TestDiff::new(50);
        let c = a + b;
        assert_eq!(c.instant, 150);
    }

    #[test]
    fn add_assign_raw_duration_to_timestamp() {
        let mut a = TestTimestamp::new(100);
        let b = TestDiff::new(50);
        a += b;
        assert_eq!(a.instant, 150);
    }

    #[test]
    fn sub_raw_duration_from_timestamp() {
        let a = TestTimestamp::new(100);
        let b = TestDiff::new(50);
        let c = a - b;
        assert_eq!(c.instant, 50);
    }

    #[test]
    fn sub_durations() {
        let a = TestDiff::new(100);
        let b = TestDiff::new(50);
        let c = a - b;
        assert_eq!(c.duration, 50);
    }

    #[test]
    fn sub_assign_durations() {
        let mut a = TestDiff::new(100);
        let b = TestDiff::new(50);
        a -= b;
        assert_eq!(a.duration, 50);
    }

    #[test]
    fn add_assign_durations() {
        let mut a = TestDiff::new(100);
        let b = TestDiff::new(50);
        a += b;
        assert_eq!(a.duration, 150);
    }

    #[test]
    fn sub_assign_raw_duration_from_timestamp() {
        let mut a = TestTimestamp::new(100);
        let b = TestDiff::new(50);
        a -= b;
        assert_eq!(a.instant, 50);
    }

    #[test]
    fn duration_multiplication() {
        let duration = TestDiff::new(10);
        let multiplied = duration * 5;
        assert_eq!(multiplied.get(), 50);
    }

    #[test]
    fn duration_multiplication_reverse() {
        let duration = TestDiff::new(10);
        let multiplied = 5 * duration;
        assert_eq!(multiplied.get(), 50);
    }

    #[test]
    fn duration_mul_assign() {
        let mut duration = TestDiff::new(10);
        duration *= 5;
        assert_eq!(duration.get(), 50);
    }

    #[test]
    fn div_durations() {
        let a = TestDiff::new(100);
        let b = 50;
        let c = a / b;
        assert_eq!(c.duration, 2);
    }

    #[test]
    fn div_assign_durations() {
        let mut a = TestDiff::new(100);
        let b = 50;
        a /= b;
        assert_eq!(a.duration, 2);
    }

    #[test]
    fn abs_duration() {
        let a = TestDiff::new(-100);
        assert_eq!(a.abs().duration, a.duration.abs());
    }

    #[rstest::rstest]
    #[case::positive(100, 200, 150)]
    #[case::negative(300, -100, 100)]
    #[case::same(100, 100, 100)]
    fn midpoint(#[case] a: i64, #[case] b: i64, #[case] expected: i64) {
        let a = TestTimestamp::new(a);
        let b = TestTimestamp::new(b);
        let midpoint = a.midpoint(b);
        assert_eq!(midpoint.instant, expected);
    }

    #[test]
    fn secs() {
        let time = Instant::from_secs(1);
        assert_eq!(time.as_seconds(), 1);
        assert_eq!(time.as_nanos(), 1_000_000_000);
    }

    #[test]
    fn nanos() {
        let time = Instant::from_nanos(1);
        assert_eq!(time.as_nanos(), 1);
    }

    #[test]
    fn millis() {
        let time = Instant::from_millis(1);
        assert_eq!(time.as_millis(), 1);
        assert_eq!(time.as_nanos(), 1_000_000);
    }

    #[test]
    fn micros() {
        let time = Instant::from_micros(1);
        assert_eq!(time.as_micros(), 1);
        assert_eq!(time.as_nanos(), 1_000);
    }

    #[test]
    fn rounding() {
        let time = Instant::from_time(1, 500_000_000);
        assert_eq!(time.as_seconds(), 2);
        assert_eq!(time.as_nanos(), 1_500_000_000);
    }

    #[test]
    fn minutes() {
        let time = Instant::from_minutes(1);
        assert_eq!(time.as_minutes(), 1);
        assert_eq!(time.as_nanos(), 60_000_000_000);
    }

    #[test]
    fn hours() {
        let time = Instant::from_hours(1);
        assert_eq!(time.as_hours(), 1);
        assert_eq!(time.as_nanos(), 3_600_000_000_000);
    }

    #[test]
    fn days() {
        let time = Instant::from_days(1);
        assert_eq!(time.as_days(), 1);
        assert_eq!(time.as_nanos(), 86_400_000_000_000);
    }

    #[test]
    fn duration_secs() {
        let time = Instant::from_secs(1);
        assert_eq!(time.as_seconds(), 1);
        assert_eq!(time.as_nanos(), 1_000_000_000);
    }

    #[test]
    fn duration_nanos() {
        let time = Instant::from_nanos(1);
        assert_eq!(time.as_nanos(), 1);
    }

    #[test]
    fn duration_millis() {
        let time = Duration::from_millis(1);
        assert_eq!(time.as_millis(), 1);
        assert_eq!(time.as_nanos(), 1_000_000);
    }

    #[test]
    fn duration_micros() {
        let time = Duration::from_micros(1);
        assert_eq!(time.as_micros(), 1);
        assert_eq!(time.as_nanos(), 1_000);
    }

    #[test]
    fn duration_truncating() {
        let time = Duration::from_nanos(1_500_000_000);
        assert_eq!(time.as_seconds_trunc(), 1);
        assert_eq!(time.as_nanos(), 1_500_000_000);
    }

    #[test]
    fn duration_minutes() {
        let time = Duration::from_minutes(1);
        assert_eq!(time.as_minutes(), 1);
        assert_eq!(time.as_nanos(), 60_000_000_000);
    }

    #[test]
    fn duration_hours() {
        let time = Duration::from_hours(1);
        assert_eq!(time.as_hours(), 1);
        assert_eq!(time.as_nanos(), 3_600_000_000_000);
    }

    #[test]
    fn duration_days() {
        let time = Duration::from_days(1);
        assert_eq!(time.as_days(), 1);
        assert_eq!(time.as_nanos(), 86_400_000_000_000);
    }

    #[test]
    fn duration_constructor() {
        let time = Duration::from_time(1, 500_000_000);
        assert_eq!(time.as_seconds_trunc(), 1);
        assert_eq!(time.as_nanos(), 1_500_000_000);
    }

    #[test]
    fn duration_seconds_f64_conversion() {
        let duration = Duration::from_seconds_f64(1.5);
        assert_eq!(duration.as_nanos(), 1_500_000_000);
        approx::assert_abs_diff_eq!(duration.as_seconds_f64(), 1.5);
    }

    #[test]
    fn duration_millis_f64_conversion() {
        let duration = Duration::from_millis_f64(1.5);
        assert_eq!(duration.as_nanos(), 1_500_000);
    }

    #[test]
    fn duration_micros_f64_conversion() {
        let duration = Duration::from_micros_f64(1.5);
        assert_eq!(duration.as_nanos(), 1_500);
    }

    #[test]
    fn duration_nanos_f64_conversion() {
        let duration = Duration::from_nanos_f64(1.5);
        assert_eq!(duration.as_nanos(), 2);
    }

    #[rstest::rstest]
    #[case::positive(Duration::from_nanos(1_400_000_000), 1, 400_000_000)]
    #[case::negative(Duration::from_nanos(-1_600_000_000), -2, 400_000_000)]
    // Take time in seconds since unix epoch * 100
    #[case::bignum(
        Duration::from_nanos(1_760_120_080_500_000_000),
        1_760_120_080,
        500_000_000
    )]
    #[case::negative_bignum(Duration::from_nanos(-1_760_120_080_500_000_000), -1_760_120_081, 500_000_000)]
    fn duration_to_timeval_micros(
        #[case] duration: Duration,
        #[case] tv_sec: i64,
        #[case] tv_usec_nanos: i64,
    ) {
        let tv = duration.to_timeval_nanos();
        assert_eq!(tv.tv_sec, tv_sec);
        assert_eq!(tv.tv_usec, tv_usec_nanos);
    }

    #[rstest]
    #[case(Duration::from_seconds_f64(1.123456789), "Duration(1.123_456_789)")]
    #[case(Duration::from_seconds_f64(1.123456), "Duration(1.123_456)")]
    #[case(Duration::from_seconds_f64(1.123), "Duration(1.123)")]
    #[case(Duration::from_secs(1234567), "Duration(1234567.0)")]
    #[case(Duration::from_seconds_f64(-1.123456), "Duration(-1.123_456)")]
    #[case(Duration::from_secs(0), "Duration(0.0)")]
    #[case(Duration::from_nanos(6), "Duration(0.000_000_006)")]
    #[case(Duration::from_micros(-1500), "Duration(-0.001_500)")]
    #[case(Duration::from_nanos(0), "Duration(0.0)")]
    fn duration_debug(#[case] duration: Duration, #[case] expected: &str) {
        assert_eq!(format!("{duration:?}"), expected);
    }

    #[rstest]
    #[case(Instant::from_nanos(1123456789), "Instant(1.123_456_789)")]
    #[case(Instant::from_nanos(1123456000), "Instant(1.123_456)")]
    #[case(Instant::from_nanos(1123000000), "Instant(1.123)")]
    #[case(Instant::from_secs(1234567), "Instant(1234567.0)")]
    #[case(Instant::from_micros(-1123456), "Instant(-1.123_456)")]
    #[case(Instant::from_secs(0), "Instant(0.0)")]
    #[case(Instant::from_nanos(6), "Instant(0.000_000_006)")]
    #[case(Instant::from_nanos(-1500), "Instant(-0.000_001_500)")]
    #[case(Instant::from_nanos(0), "Instant(0.0)")]
    fn instant_debug(#[case] instant: Instant, #[case] expected: &str) {
        assert_eq!(format!("{instant:?}"), expected);
    }

    #[test]
    fn clock_get_offset_and_rtt() {
        // Create mock clocks
        let mut mock_clock1 = MockClock::<Utc>::new();
        let mut mock_clock2 = MockClock::<Utc>::new();

        // Use a counter to track calls and return different values
        let call_count = Arc::new(Mutex::new(0));
        let call_count_clone = call_count.clone();

        // Set up expectations for clock1: first read 100, second read 300
        mock_clock1.expect_get_time().times(2).returning(move || {
            let mut count = call_count_clone.lock().unwrap();
            *count += 1;
            if *count == 1 {
                Time::new(100)
            } else {
                Time::new(300)
            }
        });

        // Set up expectations for clock2: single read 195
        mock_clock2
            .expect_get_time()
            .times(1)
            .returning(|| Time::new(195));

        // Test the get_offset_and_rtt method
        let result = mock_clock1.get_offset_and_rtt(&mock_clock2);

        // Expected calculation:
        // our_read1 = 100, their_read = 195, our_read2 = 300
        // mid = (100 + 300) / 2 = 200
        // offset = mid - their_read = 200 - 195 = 5
        // rtt = our_read2 - our_read1 = 300 - 100 = 200
        assert_eq!(result.offset().get(), 5);
        assert_eq!(result.rtt().get(), 200);
    }
}
