//! Time representation
//!
//! Simpler representation of time than `nix::time::TimeSpec`. Just abstractions over integer math

use std::{
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign},
};

use libc::timeval;

/// Abstraction used to reuse basic time arithmetic, but allow for different types based on its usage
pub trait Type {}

/// Abstraction for time type whose tick unit is approximately one femtosecond
pub trait FemtoType: Type {}

pub trait Clock<T: Type + Copy> {
    /// Read the current clock time.
    fn get_time(&self) -> Time<T>;
}

/// Abstract type for Time while keeping arithmetic consistent
///
/// This type is not usually used directly, but rather through the [`Instant`](super::Instant) and [`Tsc`](super::TscCount) types.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Time<T> {
    instant: i128,
    _marker: std::marker::PhantomData<T>,
}

impl<T> Time<T> {
    /// Get the inner value
    pub const fn get(&self) -> i128 {
        self.instant
    }
}

impl<T> From<Time<T>> for i128 {
    fn from(value: Time<T>) -> Self {
        value.instant
    }
}

// type guarded to be sealed construction
impl<T: Type> Time<T> {
    /// Zero valued epoch
    pub const EPOCH: Self = Self::new(0);

    /// constructor
    pub const fn new(instant: i128) -> Self {
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

impl<T: Type> From<i128> for Time<T> {
    fn from(value: i128) -> Self {
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

impl<T: FemtoType + Copy> Time<T> {
    pub const UNIX_EPOCH: Self = Self::new(0);
    pub const MAX: Self = Self::new(i128::MAX);
    pub const MIN: Self = Self::new(i128::MIN);

    /// Create a new `Instant` from the number of seconds since the Unix Epoch
    pub const fn from_secs(secs: i128) -> Self {
        Self::new(secs * FEMTOS_PER_SEC)
    }

    /// Create a new `Instant` from the number of milliseconds since the Unix Epoch
    pub const fn from_millis(millis: i128) -> Self {
        Self::new(millis * FEMTOS_PER_MILLI)
    }

    /// Create a new `Instant` from the number of microseconds since the Unix Epoch
    pub const fn from_micros(micros: i128) -> Self {
        Self::new(micros * FEMTOS_PER_MICRO)
    }

    /// Create a new `Instant` from the number of nanoseconds since the Unix Epoch
    pub const fn from_nanos(nanos: i128) -> Self {
        Self::new(nanos * FEMTOS_PER_NANO)
    }

    /// Create a new `Instant` from the number of picoseconds since the Unix Epoch
    pub const fn from_picos(picos: i128) -> Self {
        Self::new(picos * FEMTOS_PER_PICO)
    }

    /// Create a new `Instant` from the number of femtoseconds since the Unix Epoch
    pub const fn from_femtos(femtos: i128) -> Self {
        Self::new(femtos)
    }

    /// Create a new `Instant` from the number of minutes since the Unix Epoch
    pub const fn from_minutes(minutes: i128) -> Self {
        Self::new(minutes * FEMTOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Create a new `Instant` from the number of hours since the Unix Epoch
    pub const fn from_hours(hours: i128) -> Self {
        Self::new(hours * FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Create a new `Instant` from the number of days since the Unix Epoch
    pub const fn from_days(days: i128) -> Self {
        Self::new(days * FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }

    /// Construct from the number of seconds and nanos since the Unix Epoch
    ///
    /// # Panics
    /// Panics if `nanos >= 1_000_000_000`, or value does not fit within the type
    pub fn from_time(secs: i128, nanos: u32) -> Self {
        assert!(nanos < 1_000_000_000, "nanos must be less than 1 second");
        let secs = Self::from_secs(secs);
        let nanos = Diff::<T>::from_nanos(i128::from(nanos));
        secs + nanos
    }

    /// Returns the total number of femtoseconds since the Unix Epoch
    pub const fn as_femtos(self) -> i128 {
        self.get()
    }

    /// Returns the total number of picoseconds since the Unix Epoch
    pub const fn as_picos(self) -> i128 {
        self.get() / FEMTOS_PER_PICO
    }

    /// Returns the total number of nanoseconds since the Unix Epoch
    pub const fn as_nanos(self) -> i128 {
        self.get() / FEMTOS_PER_NANO
    }

    /// Returns the total number of microseconds, truncated, since the Unix Epoch
    pub const fn as_micros(self) -> i128 {
        self.get() / FEMTOS_PER_MICRO
    }

    /// Returns the total number of milliseconds, truncated, since the Unix Epoch
    pub const fn as_millis(self) -> i128 {
        self.get() / FEMTOS_PER_MILLI
    }

    /// Returns the total number of seconds, truncated, since the Unix Epoch
    pub const fn as_seconds(self) -> i128 {
        self.get() / FEMTOS_PER_SEC
    }

    /// Returns the total number of minutes, truncated, since the Unix Epoch
    pub const fn as_minutes(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Returns the total number of hours, truncated, since the Unix Epoch
    pub const fn as_hours(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Returns the total number of days, truncated, since the Unix Epoch
    pub const fn as_days(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }
}

/// Difference between 2 [`Time`] values
///
/// It is not recommended to use this directly, but use the [`Duration`](super::Duration) or [`TscDiff`](super::TscDiff) types
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(transparent)]
#[repr(transparent)]
pub struct Diff<T> {
    duration: i128,
    _marker: PhantomData<T>,
}

impl<T> Diff<T> {
    /// Inner Count value
    pub const fn get(&self) -> i128 {
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

impl<T> From<Diff<T>> for i128 {
    fn from(value: Diff<T>) -> Self {
        value.duration
    }
}

impl<T: Type> Diff<T> {
    /// constructor
    pub const fn new(duration: i128) -> Self {
        Self {
            duration,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> From<i128> for Diff<T> {
    fn from(value: i128) -> Self {
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
            duration: self.duration / rhs as i128,
            _marker: PhantomData,
        }
    }
}

impl<T: Type> DivAssign<usize> for Diff<T> {
    #[allow(clippy::cast_possible_wrap)]
    fn div_assign(&mut self, rhs: usize) {
        self.duration /= rhs as i128;
    }
}

impl<T: Type> Mul<usize> for Diff<T> {
    type Output = Self;

    fn mul(self, rhs: usize) -> Self::Output {
        Self {
            duration: self.duration * rhs as i128,

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
    fn mul_assign(&mut self, rhs: usize) {
        self.duration *= rhs as i128;
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

impl<T: FemtoType + Copy> Diff<T> {
    /// Create a new [`Diff`] from the number of seconds
    pub const fn from_secs(secs: i128) -> Self {
        Self::new(secs * FEMTOS_PER_SEC)
    }

    /// Create a new [`Diff`] from the number of seconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_seconds_f64(secs: f64) -> Self {
        Self::new((secs * FEMTOS_PER_SEC as f64) as i128)
    }

    /// Create a new [`Diff`] from the number of milliseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_millis_f64(millis: f64) -> Self {
        Self::new((millis * FEMTOS_PER_MILLI as f64) as i128)
    }

    /// Create a new [`Diff`] from the number of microseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_micros_f64(micros: f64) -> Self {
        Self::new((micros * FEMTOS_PER_MICRO as f64) as i128)
    }

    /// Create a new [`Diff`] from the number of nanoseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_nanos_f64(nanos: f64) -> Self {
        Self::new((nanos * FEMTOS_PER_NANO as f64) as i128)
    }

    /// Create a new [`Diff`] from the number of milliseconds
    pub const fn from_millis(millis: i128) -> Self {
        Self::new(millis * FEMTOS_PER_MILLI)
    }

    /// Create a new [`Diff`] from the number of microseconds
    pub const fn from_micros(micros: i128) -> Self {
        Self::new(micros * FEMTOS_PER_MICRO)
    }

    /// Create a new [`Diff`] from the number of nanoseconds
    pub const fn from_nanos(nanos: i128) -> Self {
        Self::new(nanos * FEMTOS_PER_NANO)
    }

    /// Create a new [`Diff`] from the number of picoseconds
    pub const fn from_picos(picos: i128) -> Self {
        Self::new(picos * FEMTOS_PER_PICO)
    }

    /// Create a new [`Diff`] from the number of femtoseconds
    pub const fn from_femtos(femtos: i128) -> Self {
        Self::new(femtos)
    }

    /// Create a new [`Diff`] from the number of minutes
    pub const fn from_minutes(minutes: i128) -> Self {
        Self::new(minutes * FEMTOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Create a new [`Diff`] from the number of hours
    pub const fn from_hours(hours: i128) -> Self {
        Self::new(hours * FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Create a new [`Diff`] from the number of days
    pub const fn from_days(days: i128) -> Self {
        Self::new(days * FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }

    /// Construct from seconds and nanos
    ///
    /// # Panics
    /// Panics if `nanos >= 1_000_000_000`, or value does not fit within the type
    pub fn from_time(secs: i128, nanos: u32) -> Self {
        assert!(nanos < 1_000_000_000, "nanos must be less than 1 second");

        let secs = Self::from_secs(secs);
        let nanos = Self::from_nanos(i128::from(nanos));
        secs + nanos
    }

    /// Returns the total number of femtoseconds
    pub const fn as_femtos(self) -> i128 {
        self.get()
    }

    /// Returns the total number of picoseconds, truncated
    pub const fn as_picos(self) -> i128 {
        self.get() / FEMTOS_PER_PICO
    }

    /// Returns the total number of nanoseconds, truncated
    pub const fn as_nanos(self) -> i128 {
        self.get() / FEMTOS_PER_NANO
    }

    /// Returns the total number of microseconds, truncated
    pub const fn as_micros(self) -> i128 {
        self.get() / FEMTOS_PER_MICRO
    }

    /// Returns the total number of milliseconds, truncated
    pub const fn as_millis(self) -> i128 {
        self.get() / FEMTOS_PER_MILLI
    }

    /// Returns the total number of seconds, truncated
    pub const fn as_seconds(self) -> i128 {
        self.get() / FEMTOS_PER_SEC
    }

    /// Returns the total number of seconds as a f64
    #[expect(clippy::cast_precision_loss, reason = "division mitigates")]
    pub const fn as_seconds_f64(self) -> f64 {
        self.get() as f64 / FEMTOS_PER_SEC as f64
    }

    /// Returns the total number of minutes, truncated
    pub const fn as_minutes(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Returns the total number of hours, truncated
    pub const fn as_hours(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Returns the total number of days, truncated
    pub const fn as_days(self) -> i128 {
        self.get() / (FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR * HOURS_PER_DAY)
    }

    /// Returns the `Duration` converted to a `timeval`.
    /// It's used by `adjtimex`/`ntp_adjtime` in the returned timestamp or in `ADJ_SETOFFSET`.
    ///
    /// A `timeval` is equivalent to `timespec` in usage, but with normally less resolution..
    /// However, a caller may supply `ADJ_NANO` to tell the kernel we want to supply a nanosecond value inside
    /// `tv_usec`, and so we do that to be consistent with our `clock_adjust` offset resolution).
    /// Thus, this does not return a correct `timeval` by the specifications of the fields themselves, but instead
    /// a `timeval` in the context of an `adjtimex` call with `ADJ_NANO` set.
    #[allow(
        clippy::cast_possible_truncation,
        reason = "tv_sec truncation should be acceptable to i64::MAX for any of our use cases, and tv_usec should be between 0 and 1e9-1 so no truncation"
    )]
    pub fn to_timeval_nanos(self) -> timeval {
        let mut tv = timeval {
            tv_sec: self.as_seconds() as i64,
            tv_usec: (self.as_nanos() % NANOS_PER_SECOND) as i64,
        };
        // Normalize the timeval, as `tv_usec` cannot be negative (so we push the "negative" place into `tv_sec`)
        if tv.tv_usec < 0 {
            tv.tv_sec -= 1;
            tv.tv_usec += 1_000_000_000_i64;
        }
        tv
    }
}

pub(crate) const FEMTOS_PER_SEC: i128 = 1_000_000_000_000_000;
pub(crate) const FEMTOS_PER_MILLI: i128 = 1_000_000_000_000;
pub(crate) const FEMTOS_PER_MICRO: i128 = 1_000_000_000;
pub(crate) const FEMTOS_PER_NANO: i128 = 1_000_000;
pub(crate) const FEMTOS_PER_PICO: i128 = 1_000;
pub(crate) const SECS_PER_MINUTE: i128 = 60;
pub(crate) const MINS_PER_HOUR: i128 = 60;
pub(crate) const HOURS_PER_DAY: i128 = 24;
pub(crate) const NANOS_PER_SECOND: i128 = 1_000_000_000;

#[cfg(test)]
mod test {
    use super::*;
    use crate::daemon::time::{Duration, Instant};

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
    fn midpoint(#[case] a: i128, #[case] b: i128, #[case] expected: i128) {
        let a = TestTimestamp::new(a);
        let b = TestTimestamp::new(b);
        let midpoint = a.midpoint(b);
        assert_eq!(midpoint.instant, expected);
    }

    #[test]
    fn secs() {
        let time = Instant::from_secs(1);
        assert_eq!(time.as_seconds(), 1);
        assert_eq!(time.as_femtos(), 1_000_000_000_000_000);
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
        assert_eq!(time.as_seconds(), 1);
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
        assert_eq!(time.as_seconds(), 1);
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
        assert_eq!(time.as_seconds(), 1);
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
        assert_eq!(duration.as_nanos(), 1);
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
}
