//! Time stamp counter (TSC) values
#![expect(clippy::cast_possible_truncation)]
#![expect(clippy::cast_precision_loss)]

use crate::daemon::time::Instant;
use crate::daemon::time::inner::FemtoType;

use super::Duration;
use super::inner::{Diff, Time};
use std::ops::Neg;
use std::{
    fmt::Display,
    ops::{Div, Mul, MulAssign},
};

use serde::{Deserialize, Serialize};

const FEMTOS_PER_SEC_F64: f64 = 1.0e15;

const FREQUENCY_TO_TIMEX_SCALE: f64 = (1 << 16) as f64;

/// Marker type to crate a raw timestamp with [`super::inner::Time`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Tsc;

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

impl std::fmt::Debug for TscCount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TscCount").field(&self.get()).finish()
    }
}

impl std::fmt::Debug for TscDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TscDiff").field(&self.get()).finish()
    }
}

impl TscCount {
    /// Get an uncorrected time value
    ///
    /// `Cu(t) = TSC(t) * p + K`, where
    /// - `Cu(t)` is the uncorrected time value, and return value
    /// - `TSC(t)` is the raw TSC value, aka self
    /// - `p` is the period of the TSC
    /// - `K` is the "epoch" of the TSC. Aka the UTC time at TSC(0)
    ///
    /// # Precision loss
    /// Precision loss can occur if Self is `> 1e15`. On modern processors, this is a year of runtime
    pub fn uncorrected_time(self, p: Period, k: Instant) -> Instant {
        k + Duration::from_seconds_f64(p.get() * self.get() as f64)
    }

    /// Calculate tick count from an uncorrected clock
    ///
    /// `Cu(t) = TSC(t) * p + K`, therefore
    ///
    /// `TSC(t) = (Cu(t) - K) / p`
    ///
    /// See [`TscCount::uncorrected_time`] for variable definitions
    pub fn from_uncorrected_time(t: Instant, p: Period, k: Instant) -> Self {
        let diff = t - k;
        let ticks = (diff.as_seconds_f64() / p.get()).round() as i128;
        Self::new(ticks)
    }
}

#[cfg(feature = "time-string-parse")]
impl std::str::FromStr for TscCount {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        i128::from_str(s).map(Self::new)
    }
}

#[cfg(feature = "time-string-parse")]
impl std::str::FromStr for TscDiff {
    type Err = std::num::ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        i128::from_str(s).map(Self::new)
    }
}

/// A frequency in Hz
///
/// ## Note on lossy-ness
/// All time durations are stored internally as `i128` values,
/// and this includes period values of ticks . This means it is possible
/// to store frequency values that will have precision loss when converted into
/// a `period` type and vice versa.
#[derive(Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Frequency(f64);

impl std::fmt::Debug for Frequency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Frequency")
            .field(&format_args!("{:.17E}", self.0))
            .finish()
    }
}

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

#[cfg(feature = "time-string-parse")]
impl std::str::FromStr for Frequency {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use nom::error::ErrorKind;

        let s = s.trim();
        let (rest, freq) =
            nom::number::complete::double::<_, (&str, ErrorKind)>(s).map_err(|e| e.to_string())?;

        if freq <= 0.0 {
            return Err("Frequency must be positive".to_string());
        }

        let suffix = rest.trim();

        match suffix.to_lowercase().as_str() {
            "" | "hz" => Ok(Self::from_hz(freq)),
            "khz" => Ok(Self::from_khz(freq)),
            "mhz" => Ok(Self::from_mhz(freq)),
            "ghz" => Ok(Self::from_ghz(freq)),
            _ => Err(format!("Unknown suffix: {suffix}")),
        }
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
        let duration_femtos = raw / rhs.0 * FEMTOS_PER_SEC_F64;
        Duration::from_femtos(duration_femtos.round() as i128)
    }
}

impl<T: FemtoType + Copy> Mul<Frequency> for Diff<T> {
    type Output = TscDiff;

    fn mul(self, rhs: Frequency) -> Self::Output {
        let duration_femtos = self.as_femtos() as f64;
        let raw = duration_femtos * rhs.0 / FEMTOS_PER_SEC_F64;
        TscDiff::new(raw.round() as i128)
    }
}

impl Mul<Duration> for Frequency {
    type Output = TscDiff;

    fn mul(self, rhs: Duration) -> Self::Output {
        rhs * self
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
    const PPB: f64 = 1.0e-9;
    const PPM: f64 = 1.0e-6;
    const PERCENT: f64 = 0.01;

    /// Construct a new skew from parts per million (ppm)
    pub const fn from_ppm(skew: f64) -> Self {
        Self(skew * Self::PPM)
    }

    /// Construct a new skew from parts per billion (ppb)
    pub const fn from_ppb(skew: f64) -> Self {
        Self(skew * Self::PPB)
    }

    /// To PPB
    ///
    /// Returns none if the value is larger than 1 billion part per billion
    pub const fn to_ppb(self) -> Option<u32> {
        let skew = self.0.abs();
        if skew < 1e9 {
            #[expect(clippy::cast_sign_loss, reason = "did abs above")]
            Some((skew / Self::PPB).round() as u32)
        } else {
            None
        }
    }

    /// Construct a new skew from percentage
    pub const fn from_percent(skew: f64) -> Self {
        Self(skew * Self::PERCENT)
    }

    /// Get the inner value
    pub const fn get(self) -> f64 {
        self.0
    }

    /// Calculate skew from 2 clocks
    ///
    /// equivalent to `1 - (num / den)`
    pub fn from_ratio(num: Period, den: Period) -> Self {
        let ratio = num.get() / den.get();
        Self(1.0 - ratio)
    }

    /// Calculate skew from period and associated error
    ///
    /// Equivalent to
    /// `Skew = error (in seconds) / period (in seconds`
    pub fn from_period_and_error(period: Period, error: Period) -> Self {
        if period.get() == 0.0 {
            return Self(0.0);
        }
        let skew = error.get() / period.get();
        Self(skew)
    }

    /// In struct timex, freq, ppsfreq, and stabil are ppm (parts per
    /// million) with a 16-bit fractional part, which means that a value
    /// of 1 in one of those fields actually means 2^-16 ppm, and
    /// 2^16=65536 is 1 ppm.  This is the case for both input values (in
    /// the case of freq) and output values.
    /// ref: See NOTES in <https://man7.org/linux/man-pages/man2/adjtimex.2.html>
    ///
    /// This function constructs a `Skew` value from a given kernel value.
    pub fn from_timex_freq(timex_freq: i64) -> Self {
        if timex_freq >= 0 {
            Self::from_ppm(timex_freq as f64 * 2.0_f64.powi(-16))
        } else {
            // i64::MAX = -i64::MIN - 1, prefer to `saturating_neg` rather
            // than overflow and wrap
            -Self::from_ppm(timex_freq.saturating_neg() as f64 * 2.0_f64.powi(-16))
        }
    }

    /// In struct timex, freq, ppsfreq, and stabil are ppm (parts per
    /// million) with a 16-bit fractional part, which means that a value
    /// of 1 in one of those fields actually means 2^-16 ppm, and
    /// 2^16=65536 is 1 ppm.  This is the case for both input values (in
    /// the case of freq) and output values.
    /// ref: See NOTES in <https://man7.org/linux/man-pages/man2/adjtimex.2.html>
    ///
    /// This function constructs a given kernel value from a `Skew` value.
    pub fn to_timex_freq(self) -> i64 {
        (FREQUENCY_TO_TIMEX_SCALE * self.0 / Self::PPM) as i64
    }

    /// `clamp` implementation delegating to inner `f64` for `Skew` values.
    ///
    /// # Panics
    /// Panics if `min > max`, `min` is NaN, or `max` is NaN.
    #[must_use]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        Self(self.get().clamp(min.get(), max.get()))
    }
}

impl Neg for Skew {
    type Output = Self;

    fn neg(self) -> Self::Output {
        Self(-self.0)
    }
}

impl Display for Skew {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ppm", self.0 / Self::PPM)
    }
}

#[cfg(feature = "time-string-parse")]
impl std::str::FromStr for Skew {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use nom::error::ErrorKind;

        let val = s.trim();
        let (rest, skew) = nom::number::complete::double::<_, (&str, ErrorKind)>(val)
            .map_err(|e| e.to_string())?;

        let suffix = rest.trim();

        match suffix.to_lowercase().as_str() {
            "" => Ok(Self(skew)),
            "%" | "percent" => Ok(Self::from_percent(skew)),
            "ppm" => Ok(Self::from_ppm(skew)),
            _ => Err(format!("Unknown suffix: {suffix}")),
        }
    }
}

/// A representation of a TSC clock period in seconds
///
/// Logically, this value is the mathematical inverse of [`Frequency`]. In other words,
/// `[Period] = 1 / [Frequency]`
///
/// ## Note on lossy-ness
/// All time durations are stored internally as `i128` values. This means it is possible
/// to store frequency values that will have precision loss when converted from
/// measurements based on [`Duration`] types
///
/// ## Note on zero valued periods
/// While not logical for clocks, it can come up for error calculations currently.
/// FIXME, does a zero error ever make sense?
#[derive(Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Period(f64);

impl std::fmt::Debug for Period {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Period")
            .field(&format_args!("{:.17E}", self.0))
            .finish()
    }
}

impl Period {
    /// Construct from seconds
    ///
    /// # Panics
    /// Panics if `seconds` < 0
    pub fn from_seconds(seconds: f64) -> Self {
        assert!(seconds >= 0.0);
        Self(seconds)
    }

    /// Construct from a duration
    ///
    /// # Panics
    /// Panics if `duration < 0`
    pub fn from_duration(duration: Duration) -> Self {
        assert!(duration.get() >= 0);
        Self(duration.as_seconds_f64())
    }

    /// Get the inner duration in seconds
    pub fn get(self) -> f64 {
        self.0
    }

    /// Construct from a [`Frequency`]
    ///
    /// # Precision loss
    /// Given that there is a floating point to integer conversion, precision loss can be
    /// seen from either large (> 1 PHz) or small (< 1 Hz) frequency values.
    pub fn from_frequency(frequency: Frequency) -> Self {
        Self::from_seconds(1.0 / frequency.get())
    }
}

impl std::fmt::Display for Period {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:.e}s", self.0)
    }
}

impl Mul<Period> for TscDiff {
    type Output = Duration;

    fn mul(self, rhs: Period) -> Self::Output {
        let dur_seconds = self.get() as f64 * rhs.get();
        Duration::from_seconds_f64(dur_seconds)
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
        let period = self.as_seconds_f64() / rhs.get() as f64;
        Period::from_seconds(period)
    }
}

impl Div<Period> for Duration {
    type Output = TscDiff;

    fn div(self, rhs: Period) -> Self::Output {
        let diff = self.as_seconds_f64() / rhs.get();
        TscDiff::new(diff.round() as i128)
    }
}

#[cfg(feature = "time-string-parse")]
impl std::str::FromStr for Period {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use nom::error::ErrorKind;

        let val = s.trim();
        let (rest, period) = nom::number::complete::double::<_, (&str, ErrorKind)>(val)
            .map_err(|e| e.to_string())?;

        if period <= 0.0 {
            return Err("Period must be positive".to_string());
        }

        let suffix = rest.trim();

        match suffix.to_lowercase().as_str() {
            "" | "s" | "sec" | "second" | "seconds" => Ok(Self(period)),
            _ => Err(format!("Unknown suffix: {suffix}")),
        }
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
        assert_abs_diff_eq!(period.get(), 0.1);
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
    fn skew_to_timex_freq() {
        let skew = Skew::from_ppm(1.0);
        assert_eq!(skew.to_timex_freq(), 65536);
        let skew = Skew::from_ppm(2.0);
        assert_eq!(skew.to_timex_freq(), 131072);
        let skew = Skew::from_ppm(-1.0);
        assert_eq!(skew.to_timex_freq(), -65536);
        let skew = Skew::from_ppm(-1.5);
        assert_eq!(skew.to_timex_freq(), -98304);
        let skew = Skew::from_ppm(0.0);
        assert_eq!(skew.to_timex_freq(), 0);
        let skew = Skew::from_ppm(f64::MAX);
        assert_eq!(skew.to_timex_freq(), i64::MAX);
        let skew = Skew::from_ppm(f64::MIN);
        assert_eq!(skew.to_timex_freq(), i64::MIN);
    }

    #[test]
    fn skew_from_timex_freq() {
        let skew = Skew::from_timex_freq(65536);
        assert_abs_diff_eq!(skew.get(), 1.0 * 1.0e-6);
        let skew = Skew::from_timex_freq(-65536);
        assert_abs_diff_eq!(skew.get(), -1.0 * 1.0e-6);
        let skew = Skew::from_timex_freq(98304);
        assert_abs_diff_eq!(skew.get(), 1.5 * 1.0e-6);
        let skew = Skew::from_timex_freq(-98304);
        assert_abs_diff_eq!(skew.get(), -1.5 * 1.0e-6);
        let skew = Skew::from_timex_freq(0);
        assert_abs_diff_eq!(skew.get(), 0.0);
        let skew = Skew::from_timex_freq(i64::MAX);
        assert_abs_diff_eq!(
            skew.get(),
            i64::MAX as f64 / (65536.0 * 1e6),
            epsilon = 0.11
        );
        let skew = Skew::from_timex_freq(i64::MIN);
        assert_abs_diff_eq!(
            skew.get(),
            -i64::MAX as f64 / (65536.0 * 1e6),
            epsilon = 0.11
        );
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
        assert_abs_diff_eq!(period.get(), 0.001);
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

    #[test]
    fn period_display() {
        let period = Period::from_seconds(1e-9);
        assert_eq!(period.to_string(), "1e-9s");
    }

    #[test]
    fn uncorrected_time() {
        let tsc = TscCount::new(1_000_000_000);
        let p = Period::from_seconds(1.0e-9);
        let k = Instant::from_days(365);

        let uncorrected = tsc.uncorrected_time(p, k);
        assert_eq!(
            uncorrected,
            Instant::from_days(365) + Duration::from_secs(1)
        );
    }

    #[test]
    fn from_uncorrected_time() {
        let p = Period::from_seconds(1.0e-9);
        let k = Instant::from_days(365);

        let uncorrected = Instant::from_days(365) + Duration::from_secs(1);
        let tsc = TscCount::from_uncorrected_time(uncorrected, p, k);
        assert_eq!(tsc.get(), 1_000_000_000);
    }

    #[cfg(feature = "time-string-parse")]
    #[rstest]
    #[case("1.0", 1.0)]
    #[case("1 Hz", 1.0)]
    #[case("1 kHz", 1000.0)]
    #[case("1 MHz", 1_000_000.0)]
    #[case("1ghz", 1_000_000_000.0)]
    fn frequency_parse_from_str_valid(#[case] input: &str, #[case] expected: f64) {
        use std::str::FromStr;
        let freq = Frequency::from_str(input).unwrap();
        assert_abs_diff_eq!(freq.get(), expected);
    }

    #[cfg(feature = "time-string-parse")]
    #[rstest]
    #[case("")]
    #[case("invalid")]
    #[case::negative("-1 Hz")]
    #[case("1 InvalidUnit")]
    fn frequency_parse_from_str_invalid(#[case] input: &str) {
        use std::str::FromStr;
        let _ = Frequency::from_str(input).unwrap_err();
    }

    #[cfg(feature = "time-string-parse")]
    #[rstest]
    #[case("0.001", 0.001)]
    #[case("100 ppm", 0.0001)]
    #[case("5%", 0.05)]
    #[case("5 percent", 0.05)]
    fn skew_parse_from_str(#[case] input: &str, #[case] expected: f64) {
        use std::str::FromStr;
        let skew = Skew::from_str(input).unwrap();
        assert_abs_diff_eq!(skew.get(), expected);
    }

    #[cfg(feature = "time-string-parse")]
    #[rstest]
    #[case("1.0", 1.0)]
    #[case("1s", 1.0)]
    #[case("0.0000000000000001seconds", 0.000_000_000_000_000_1)]
    #[case("0.001 sec", 0.001)]
    #[case("1000second", 1000.0)]
    fn period_parse_from_str_valid(#[case] input: &str, #[case] expected: f64) {
        use std::str::FromStr;
        let freq = Period::from_str(input).unwrap();
        assert_abs_diff_eq!(freq.get(), expected);
    }

    #[cfg(feature = "time-string-parse")]
    #[rstest]
    #[case("")]
    #[case("invalid")]
    #[case::negative("-1 Hz")]
    #[case("1 InvalidUnit")]
    fn period_parse_from_str_invalid(#[case] input: &str) {
        use std::str::FromStr;
        let _ = Period::from_str(input).unwrap_err();
    }

    #[test]
    fn debug_tsc_count() {
        let tsc = TscCount::new(1_000_000_000);
        assert_eq!(format!("{tsc:?}"), "TscCount(1000000000)");
    }

    #[test]
    fn debug_tsc_diff() {
        let diff = TscDiff::new(1_000_000_000);
        assert_eq!(format!("{diff:?}"), "TscDiff(1000000000)");
    }

    #[test]
    fn duration_div_by_tsc_diff() {
        let expected_period = 1.0 / 3.3e9; // 3.3GHz
        let one_second = 1.0_f64;
        let tsc_diff = one_second / expected_period;
        let tsc_diff = TscDiff::new(tsc_diff.round() as i128);

        let one_second = Duration::from_seconds_f64(one_second);
        let period = one_second / tsc_diff;

        approx::assert_abs_diff_eq!(period.get(), expected_period);
    }
}
