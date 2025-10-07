//! NTP Short implementation

use chrono::TimeDelta;
use nom::Parser;
use std::time::Duration;

use crate::daemon::time::Duration as ClockBoundDuration;

/// NTP Short value
///
/// See [RFC 5905](https://datatracker.ietf.org/doc/html/rfc5905#section-6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(transparent)]
pub struct Short {
    // stored in native endian
    inner: u32,
}

impl Short {
    /// Maximum spec-supported value
    pub const MAX: Self = Self { inner: 0xFFFF_FFFF };
    /// Maximum spec-supported value in seconds
    pub const MAX_SEC: f64 = Self::to_secs_f64(Self::MAX);
    /// Zero value
    pub const ZERO: Self = Self { inner: 0 };

    /// Minimum subsecond difference this type can represent, in seconds
    pub const EPSILON: f64 = 1.0f64 / (1u64 << 16) as f64;

    /// Construct from a native endian representation value
    pub const fn new(ne: u32) -> Self {
        Self { inner: ne }
    }

    /// Convert to seconds
    pub const fn to_secs_f64(self) -> f64 {
        self.inner as f64 / 65536.0
    }

    /// Convert from seconds
    ///
    /// Returns None if out of bounds or negative
    pub fn from_secs_f64(secs: f64) -> Option<Self> {
        (0.0..=Self::MAX_SEC).contains(&secs).then(|| {
            let inner = (secs * 65536.0).ceil() as u32;
            Self { inner }
        })
    }

    /// Convert to time delta
    pub fn to_time_delta(self) -> TimeDelta {
        let seconds = self.inner >> 16;
        let fraction = self.inner & 0xFFFF;
        let nanoseconds = f64::from(fraction) / f64::from(1u32 << 16) * 1e9f64;
        let nanoseconds = nanoseconds as u32;
        // unwrap ok. TimeDelta encompasses all possible time of Short
        TimeDelta::new(i64::from(seconds), nanoseconds).unwrap()
    }

    /// Convert to network endian representation
    pub fn into_network_endian(self) -> u32 {
        self.inner.to_be()
    }

    /// Parse from bytes
    pub fn parse(input: &[u8]) -> nom::IResult<&[u8], Self> {
        nom::number::be_u32()
            .parse(input)
            .map(|(input, val)| (input, Self::new(val)))
    }

    /// panics if not within expected error bounds
    #[cfg(test)]
    pub fn assert_within_error_bound(self, expected: TimeDelta) {
        let error = self.to_time_delta() - expected;
        let error = error.abs().to_std().unwrap().as_secs_f64();

        if error > Self::EPSILON {
            panic!(
                "error when comparing. expected: {expected}, actual: {0}",
                self.to_time_delta()
            );
        }
    }
}

impl From<Short> for Duration {
    fn from(value: Short) -> Self {
        let secs = value.to_secs_f64();
        Duration::from_secs_f64(secs)
    }
}

impl TryFrom<Duration> for Short {
    type Error = OutOfRangeError;

    fn try_from(value: Duration) -> Result<Self, Self::Error> {
        Self::from_secs_f64(value.as_secs_f64()).ok_or(OutOfRangeError)
    }
}

impl From<Short> for ClockBoundDuration {
    fn from(value: Short) -> Self {
        let secs = value.to_secs_f64();
        Self::from_seconds_f64(secs)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfRangeError;

impl std::fmt::Display for OutOfRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NTP out of range")
    }
}

#[cfg(test)]
mod test {
    use hex_literal::hex;

    use super::*;

    #[test]
    fn negative_is_none() {
        let value = Short::from_secs_f64(-1.0);
        assert_eq!(value, None);
    }

    #[test]
    fn above_max_saturates() {
        let secs = Duration::from_secs_f64(u16::MAX as f64 + 1.0);
        Short::try_from(secs).unwrap_err();
    }

    #[test]
    fn seconds() {
        let secs = 15;
        let duration = Duration::from_secs(secs as u64);
        let value = Short::try_from(duration).unwrap();

        assert_eq!(value.inner, secs << 16);
    }

    #[rstest::rstest]
    #[case::minimal(1.0 / (1 << 17) as f64, 1)]
    #[case::maximal(1.0 - 1.0 / ( 1 << 17) as f64, 1 << 16)]
    fn fraction(#[case] secs: f64, #[case] expected_inner: u32) {
        let value = Short::from_secs_f64(secs).unwrap();
        assert_eq!(value.inner, expected_inner);
    }

    #[test]
    fn to_secs() {
        let ntp_value = Short { inner: 3 << 16 };
        assert_eq!(ntp_value.to_secs_f64(), 3.0);
    }

    // Root Delay: 0.000031 seconds. Value taken from a wireshark capture
    const ROOT_DELAY: &[u8] = &hex!("00000002");
    const ROOT_DELAY_SECS: f64 = 0.000031;

    #[test]
    fn parse() {
        let (leftover, parsed) = Short::parse(ROOT_DELAY).unwrap();
        assert!(leftover.is_empty());
        // NTP short is only SO accurate
        let allowed_error = 1f64 / (1 << 16) as f64;
        let error = (parsed.to_secs_f64() - ROOT_DELAY_SECS).abs();
        assert!(error <= allowed_error);
    }

    #[test]
    fn to_time_delta() {
        let value = Short::from_secs_f64(15.00078).unwrap();
        // loses accuracy when going to Short
        let float_value = value.to_secs_f64();
        let time_delta = value.to_time_delta();
        let time_delta_secs = time_delta.to_std().unwrap().as_secs_f64();
        // time delta is accurate to nanoseconds. F64 has roughly 15.95 digits worth of accuracy.
        // Given that the maximum value that Short can realistically hold is less than 65_535.999_999_999,
        // then these values should be accurate to +/- 1 nanosecond
        let allowable_error = 0.000_000_001;
        let error = (time_delta_secs - float_value).abs();
        assert!(error <= allowable_error);
    }

    #[test]
    fn max_value() {
        let value = Short::from_secs_f64(Short::MAX_SEC).unwrap();
        assert_eq!(value, Short::MAX);
    }

    #[test]
    fn from_clock_bound_duration() {
        let short = Short::from_secs_f64(11.123).unwrap();
        let duration = ClockBoundDuration::from(short);

        assert_eq!(duration.as_seconds_f64(), short.to_secs_f64());
    }
}
