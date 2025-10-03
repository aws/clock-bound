//! A simplified time type for `ClockBound`

use super::inner::{Diff, Time};

/// Marker type to signify a time as a timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utc;

impl crate::private::Sealed for Utc {}
impl super::inner::Type for Utc {}

/// Representation of an absolute time timestamp
///
/// This value represents the number of **femto**seconds since epoch, without leap seconds.
/// A femtosecond is 1/1,000,000,000,000,000th of a second. or 1e-15
///
/// This type's epoch is January 1, 1970 0:00:00 UTC (aka "UNIX timestamp")
///
/// This type's inner value is an i128 number of femtoseconds from epoch.
pub type Instant = Time<Utc>;

/// The corresponding duration type for [`Instant`]
pub type Duration = Diff<Utc>;

impl Instant {
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
        let nanos = Duration::from_nanos(i128::from(nanos));
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

impl Duration {
    /// Create a new [`Duration`] from the number of seconds
    pub const fn from_secs(secs: i128) -> Self {
        Self::new(secs * FEMTOS_PER_SEC)
    }

    /// Create a new [`Duration`] from the number of seconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_seconds_f64(secs: f64) -> Self {
        Self::new((secs * FEMTOS_PER_SEC as f64) as i128)
    }

    /// Create a new [`Duration`] from the number of milliseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_millis_f64(millis: f64) -> Self {
        Self::new((millis * FEMTOS_PER_MILLI as f64) as i128)
    }

    /// Create a new [`Duration`] from the number of microseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_micros_f64(micros: f64) -> Self {
        Self::new((micros * FEMTOS_PER_MICRO as f64) as i128)
    }

    /// Create a new [`Duration`] from the number of nanoseconds in `f64` format
    ///
    /// Will truncate to the nearest femtosecond
    #[expect(clippy::cast_possible_truncation, reason = "truncation documented")]
    #[expect(clippy::cast_precision_loss, reason = "const will not wrap")]
    pub const fn from_nanos_f64(nanos: f64) -> Self {
        Self::new((nanos * FEMTOS_PER_NANO as f64) as i128)
    }

    /// Create a new [`Duration`] from the number of milliseconds
    pub const fn from_millis(millis: i128) -> Self {
        Self::new(millis * FEMTOS_PER_MILLI)
    }

    /// Create a new [`Duration`] from the number of microseconds
    pub const fn from_micros(micros: i128) -> Self {
        Self::new(micros * FEMTOS_PER_MICRO)
    }

    /// Create a new [`Duration`] from the number of nanoseconds
    pub const fn from_nanos(nanos: i128) -> Self {
        Self::new(nanos * FEMTOS_PER_NANO)
    }

    /// Create a new [`Duration`] from the number of picoseconds
    pub const fn from_picos(picos: i128) -> Self {
        Self::new(picos * FEMTOS_PER_PICO)
    }

    /// Create a new [`Duration`] from the number of femtoseconds
    pub const fn from_femtos(femtos: i128) -> Self {
        Self::new(femtos)
    }

    /// Create a new [`Duration`] from the number of minutes
    pub const fn from_minutes(minutes: i128) -> Self {
        Self::new(minutes * FEMTOS_PER_SEC * SECS_PER_MINUTE)
    }

    /// Create a new [`Duration`] from the number of hours
    pub const fn from_hours(hours: i128) -> Self {
        Self::new(hours * FEMTOS_PER_SEC * SECS_PER_MINUTE * MINS_PER_HOUR)
    }

    /// Create a new [`Duration`] from the number of days
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
}

pub(crate) const FEMTOS_PER_SEC: i128 = 1_000_000_000_000_000;
pub(crate) const FEMTOS_PER_MILLI: i128 = 1_000_000_000_000;
pub(crate) const FEMTOS_PER_MICRO: i128 = 1_000_000_000;
pub(crate) const FEMTOS_PER_NANO: i128 = 1_000_000;
pub(crate) const FEMTOS_PER_PICO: i128 = 1_000;
pub(crate) const SECS_PER_MINUTE: i128 = 60;
pub(crate) const MINS_PER_HOUR: i128 = 60;
pub(crate) const HOURS_PER_DAY: i128 = 24;

#[cfg(test)]
mod test {
    use super::*;

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
}
