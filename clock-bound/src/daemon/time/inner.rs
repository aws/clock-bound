//! Time representation
//!
//! Simpler representation of time than `nix::time::TimeSpec`. Just abstractions over integer math

use std::{
    marker::PhantomData,
    ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Sub, SubAssign},
};

/// Abstraction used to reuse basic time arithmetic, but allow for different types based on its usage
pub trait Type: crate::private::Sealed {}

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

#[cfg(test)]
mod test {
    use super::*;

    #[derive(Clone, Copy)]
    struct TestType;

    impl crate::private::Sealed for TestType {}
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
}
