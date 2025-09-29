//! Time stamp counter (TSC) values
#![expect(clippy::cast_possible_truncation)]
#![expect(clippy::cast_precision_loss)]

use super::Duration;
use super::inner::{Diff, Time};
use std::{
    fmt::Display,
    ops::{Div, Mul, MulAssign},
};

use serde::{Deserialize, Serialize};

/// Marker type to crate a raw timestamp with [`super::inner::Time`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tsc;

impl crate::private::Sealed for Tsc {}
impl super::inner::Type for Tsc {}

/// Abstract representation of a time stamp counter.
///
/// The way this value used is that the difference between 2 [`TscCount`] values
/// is some number of ticks. And then `a priori` or derived knowledge of the time source can be
/// used to convert this difference into a span of time
///
/// This value could come from various different forms, for example
/// from a `CLOCK_MONOTONIC_RAW` `clock_gettime` read, or reading a TSC via `rdtsc` on x86 platforms.
///
/// This value has no unit aside from `Count`. It is the job of clock sync algorithms to convert this meaningfully into time.
pub type TscCount = Time<Tsc>;

/// Corresponding duration type for [`TscCount`]
pub type TscDiff = Diff<Tsc>;

/// A frequency in Hz
///
/// ## Note on lossy-ness
/// All time durations are stored internally as `i128` values,
/// and this includes period values of ticks . This means it is possible
/// to store frequency values that will have precision loss when converted into
/// a `period` type and vice versa.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Frequency(f64);

impl Frequency {
    /// Get inner value in hz
    pub fn get(self) -> f64 {
        self.0
    }

    /// Construct from Ghz
    ///
    /// # Panics
    /// Panics if the input is not positive
    pub fn from_ghz(ghz: f64) -> Self {
        Self::from_hz(ghz * 1_000_000_000.0)
    }

    /// Construct from Mhz
    ///
    /// # Panics
    /// Panics if the input is not positive
    pub fn from_mhz(mhz: f64) -> Self {
        Self::from_hz(mhz * 1_000_000.0)
    }

    /// Construct From Khz
    ///
    /// # Panics
    /// Panics if the input is not positive
    pub fn from_khz(khz: f64) -> Self {
        Self::from_hz(khz * 1_000.0)
    }

    /// Construct from Hz
    ///
    /// # Panics
    /// Panics if the input is not positive
    pub fn from_hz(hz: f64) -> Self {
        assert!(hz > 0.0);
        Self(hz)
    }

    /// Convert into a [`Period`]
    pub fn period(self) -> Period {
        Period::from_frequency(self)
    }
}

impl Display for Frequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Hz", self.0)
    }
}

impl TryFrom<f64> for Frequency {
    type Error = &'static str;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if value <= 0.0 {
            Err("Frequency must be positive")
        } else {
            Ok(Self(value))
        }
    }
}

impl Div<Frequency> for TscDiff {
    type Output = Duration;

    fn div(self, rhs: Frequency) -> Self::Output {
        let raw = self.get() as f64;
        let duration_femtos = raw / rhs.0 * 1.0e15;
        Duration::from_femtos(duration_femtos as i128)
    }
}

impl Mul<Frequency> for Duration {
    type Output = TscDiff;

    fn mul(self, rhs: Frequency) -> Self::Output {
        let duration_femtos = self.as_femtos() as f64;
        let raw = duration_femtos * rhs.0 / 1.0e15;
        TscDiff::new(raw as i128)
    }
}

impl Mul<f64> for Frequency {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Mul<Frequency> for f64 {
    type Output = Frequency;

    fn mul(self, rhs: Frequency) -> Self::Output {
        rhs * self
    }
}

impl MulAssign<f64> for Frequency {
    fn mul_assign(&mut self, rhs: f64) {
        self.0 *= rhs;
    }
}

/// A convenience type to denoting skew
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default, Serialize, Deserialize)]
pub struct Skew(f64);

impl Skew {
    const PPM: f64 = 1.0e-6;
    const PERCENT: f64 = 0.01;

    /// Construct a new skew from parts per million (ppm)
    pub fn from_ppm(skew: f64) -> Self {
        Self(skew * Self::PPM)
    }

    /// Construct a new skew from percentage
    pub fn from_percent(skew: f64) -> Self {
        Self(skew * Self::PERCENT)
    }

    /// Get the inner value
    pub fn get(self) -> f64 {
        self.0
    }
}

impl Display for Skew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ppm", self.0 / Self::PPM)
    }
}

/// A representation of a TSC clock period
///
/// Logically, this value is the mathematical inverse of [`Frequency`]. In other words,
/// `[Period] = 1 / [Frequency]`
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Period(Duration);

impl Period {
    /// Construct from a duration
    ///
    /// # Panics
    /// Panics if `duration <= 0`
    pub fn from_duration(duration: Duration) -> Self {
        assert!(duration.get() > 0);
        Self(duration)
    }

    /// Get the inner duration
    pub fn get(self) -> Duration {
        self.0
    }

    /// Construct from a [`Frequency`]
    ///
    /// # Precision loss
    /// Given that there is a floating point to integer conversion, precision loss can be
    /// seen from either large (> 1 PHz) or small (< 1 Hz) frequency values.
    pub fn from_frequency(frequency: Frequency) -> Self {
        // no assert statement. Frequency can't be negative
        let period_femtos = 1.0e15 / frequency.0;
        let period_femtos = period_femtos.round() as i128;
        let period = Duration::from_femtos(period_femtos);
        Self::from_duration(period)
    }
}

impl Mul<Period> for TscDiff {
    type Output = Duration;

    fn mul(self, rhs: Period) -> Self::Output {
        let dur = self.get() * rhs.get().as_femtos();
        Duration::from_femtos(dur)
    }
}

impl Mul<TscDiff> for Period {
    type Output = Duration;

    fn mul(self, rhs: TscDiff) -> Self::Output {
        rhs * self
    }
}

impl Div<TscDiff> for Duration {
    type Output = Period;

    fn div(self, rhs: TscDiff) -> Self::Output {
        let period = self.as_femtos() / rhs.get();
        Period::from_duration(Duration::from_femtos(period))
    }
}

impl Div<Period> for Duration {
    type Output = TscDiff;

    fn div(self, rhs: Period) -> Self::Output {
        let diff = self.as_femtos() / rhs.get().as_femtos();
        TscDiff::new(diff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_abs_diff_eq;
    use rstest::rstest;

    #[test]
    #[expect(clippy::similar_names)]
    fn frequency_conversions() {
        let f_ghz = Frequency::from_ghz(1.0);
        let f_mhz = Frequency::from_mhz(1000.0);
        let f_khz = Frequency::from_khz(1_000_000.0);
        let f_hz = Frequency::from_hz(1_000_000_000.0);

        assert_abs_diff_eq!(f_ghz.0, 1_000_000_000.0);
        assert_abs_diff_eq!(f_mhz.0, 1_000_000_000.0);
        assert_abs_diff_eq!(f_khz.0, 1_000_000_000.0);
        assert_abs_diff_eq!(f_hz.0, 1_000_000_000.0);
    }

    #[test]
    fn frequency_period() {
        let f = Frequency::from_hz(10.0); // 10 Hz = 0.1 seconds period
        let period = f.period();
        assert_eq!(period.get(), Duration::from_millis(100));
    }

    #[test]
    fn tsc_diff_div_frequency() {
        let diff = TscDiff::new(1000);
        let freq = Frequency::from_hz(100.0);
        let result = diff / freq;

        // 1000 ticks at 100Hz = 10 seconds
        assert_eq!(result, Duration::from_secs(10));
    }

    #[test]
    fn duration_mul_frequency() {
        let duration = Duration::from_secs(1);
        let freq = Frequency::from_hz(100.0);
        let result = duration * freq;

        // 1 second at 100Hz = 100 ticks
        assert_eq!(result.get(), 100);
    }

    #[rstest]
    #[case(1.0, true)]
    #[case(0.0, false)]
    #[case(-1.0, false)]
    fn frequency_validation(#[case] frequency: f64, #[case] is_ok: bool) {
        assert_eq!(Frequency::try_from(frequency).is_ok(), is_ok);
    }

    #[test]
    fn skew_from_ppm() {
        let skew = Skew::from_ppm(100.0);
        assert_abs_diff_eq!(skew.get(), 100.0 * 1.0e-6);
    }

    #[test]
    fn skew_from_percent() {
        let skew = Skew::from_percent(5.0);
        assert_abs_diff_eq!(skew.get(), 0.05);
    }

    #[test]
    fn skew_display() {
        let skew = Skew::from_ppm(100.0);
        assert_eq!(skew.to_string(), "100 ppm");
    }

    #[test]
    fn tsc_diff_mul_period() {
        let tsc_diff = TscDiff::new(1000);
        let period = Period::from_duration(Duration::from_millis(10));
        let result = tsc_diff * period;

        assert_eq!(result, Duration::from_secs(10));
    }

    #[test]
    fn duration_div_period() {
        let tsc_diff = Duration::from_secs(1);
        let period = Period::from_duration(Duration::from_millis(10));
        let result = tsc_diff / period;

        assert_eq!(result.get(), 100);
    }

    #[test]
    fn frequency_multiplication() {
        let freq = Frequency::from_hz(100.0);
        let result = freq * 2.0;
        assert_abs_diff_eq!(result.get(), 200.0);

        let result = 2.0 * freq;
        assert_abs_diff_eq!(result.get(), 200.0);
    }

    #[test]
    fn frequency_mul_assign() {
        let mut freq = Frequency::from_hz(100.0);
        freq *= 2.0;
        assert_abs_diff_eq!(freq.get(), 200.0);
    }

    #[test]
    fn period_from_frequency() {
        let freq = Frequency::from_hz(1000.0);
        let period = Period::from_frequency(freq);
        assert_eq!(period.get(), Duration::from_micros(1000));
    }

    #[test]
    fn test_duration_period_operations() {
        let duration = Duration::from_secs(2);
        let period = Period::from_duration(Duration::from_millis(500));

        // Test duration / period
        let tsc_diff = duration / period;
        assert_eq!(tsc_diff.get(), 4); // 2 seconds / 500ms = 4 ticks

        // Test reverse operation
        let result_duration = tsc_diff * period;
        assert_eq!(result_duration, duration);
    }

    #[test]
    fn skew_calculations() {
        let ppm_skew = Skew::from_ppm(100.0);
        let percent_skew = Skew::from_percent(1.0);

        assert_abs_diff_eq!(ppm_skew.get(), 100.0e-6);
        assert_abs_diff_eq!(percent_skew.get(), 0.01);
    }
}
