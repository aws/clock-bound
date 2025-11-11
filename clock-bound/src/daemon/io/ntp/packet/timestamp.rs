//! NTP Timestamp implementation

use chrono::{DateTime, TimeDelta, Utc};
use nom::Parser;

use crate::daemon::time::Instant as ClockBoundInstant;

/// NTP Timestamp value
///
/// See [RFC 5905](https://datatracker.ietf.org/doc/html/rfc5905#section-6)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
#[repr(transparent)]
pub struct Timestamp {
    // stored in native endian
    inner: u64,
}

impl Timestamp {
    /// Maximum spec-supported value
    pub const MAX: Self = Self { inner: u64::MAX };

    /// Zero value
    pub const ZERO: Self = Self { inner: 0 };

    /// Minimum subsecond difference this type can represent, in seconds
    pub const EPSILON: f64 = 1.0f64 / (1u64 << 32) as f64;

    /// Maximum spec-supported value as seconds since the NTP epoch for 1900
    pub const fn max_seconds_since_ntp_epoch() -> f64 {
        let td = Self::MAX.to_time_delta_since_ntp_epoch();
        let secs = td.num_seconds() as f64;
        let subsecs = td.subsec_nanos() as f64 / 1e9f64;
        secs + subsecs
    }

    /// Get the inner value
    pub const fn get(self) -> u64 {
        self.inner
    }

    // From https://stackoverflow.com/questions/29112071/how-to-convert-ntp-time-to-unix-epoch-time-in-c-language-linux
    //
    // Unix uses an epoch located at 1/1/1970-00:00h (UTC) and NTP uses 1/1/1900-00:00h.
    // This leads to an offset equivalent to 70 years in seconds.
    // there are 17 leap years between the two dates so the offset is
    // (70*365 + 17)*86400 = 2208988800
    const NTP_UNIX_EPOCH_DIFFERENCE_CHRONO: chrono::TimeDelta =
        chrono::TimeDelta::new(2_208_988_800, 0).unwrap();

    /// Construct from a native endian representation
    pub const fn new(val: u64) -> Self {
        Self { inner: val }
    }

    /// Convert to time since the NTP epoch
    ///
    /// NTP epoch is 1/1/1900-00:00h.
    /// NOTE that this is different from the UNIX epoch at 1970
    pub const fn to_time_delta_since_ntp_epoch(self) -> TimeDelta {
        const FRACTION_TO_NANO: f64 = 1e9f64 / (1u64 << 32) as f64;
        let seconds = self.inner >> 32;
        let fraction = self.inner & 0xFFFF_FFFF;
        let nanoseconds = fraction as f64 * FRACTION_TO_NANO;
        // round down on Self -> float conversions. Opposite in other direction.
        let nanoseconds = nanoseconds as u32;
        // Unwrap ok. TimeDelta encompasses all of Timestamp
        TimeDelta::new(seconds as i64, nanoseconds).unwrap()
    }

    /// Convert to time since UNIX epoch
    pub fn to_time_delta_since_unix_epoch(self) -> TimeDelta {
        let ntp_time = self.to_time_delta_since_ntp_epoch();
        ntp_time - Self::NTP_UNIX_EPOCH_DIFFERENCE_CHRONO
    }

    /// Convert to UTC datetime
    pub fn to_utc_datetime(self) -> DateTime<Utc> {
        let secs = self.to_time_delta_since_unix_epoch();
        DateTime::<Utc>::from_timestamp(secs.num_seconds(), secs.subsec_nanos().unsigned_abs())
            .unwrap()
    }

    /// Convert UTC datetime to ntp timestamp
    ///
    /// Returns None if out of range
    pub fn from_utc_system(val: chrono::DateTime<Utc>) -> Option<Self> {
        let seconds = val.timestamp() + Self::NTP_UNIX_EPOCH_DIFFERENCE_CHRONO.num_seconds();
        let nanoseconds = f64::from(val.timestamp_subsec_nanos()) / 1e9;
        let seconds = seconds as f64 + nanoseconds;
        Self::from_secs_since_ntp_epoch(seconds)
    }

    /// Convert from seconds
    pub fn from_secs_since_ntp_epoch(secs: f64) -> Option<Self> {
        (0.0..=Self::max_seconds_since_ntp_epoch())
            .contains(&secs)
            .then(|| {
                // round up when going from f64 -> Self. Down in opposite direction
                let inner = (secs * (1u64 << 32) as f64).ceil() as u64;
                Self { inner }
            })
    }

    /// Convert to network endian representation
    pub fn into_network_endian(self) -> u64 {
        self.inner.to_be()
    }

    /// Parse from bytes of an NTP packet
    pub fn parse(input: &[u8]) -> nom::IResult<&[u8], Self> {
        nom::number::be_u64()
            .parse(input)
            .map(|(input, val)| (input, Self::new(val)))
    }

    /// panics if not within expected error bounds
    #[cfg(test)]
    pub fn assert_within_error_bound(self, expected: DateTime<Utc>) {
        let error = self.to_utc_datetime() - expected;
        let error = error.abs().to_std().unwrap().as_secs_f64();

        if error > Self::EPSILON {
            panic!(
                "error when comparing. expected: {expected}, actual: {0}",
                self.to_utc_datetime()
            );
        }
    }
}

impl From<Timestamp> for DateTime<Utc> {
    fn from(value: Timestamp) -> Self {
        value.to_utc_datetime()
    }
}

impl From<Timestamp> for ClockBoundInstant {
    fn from(value: Timestamp) -> Self {
        let dt = value.to_utc_datetime();
        Self::from_time(i128::from(dt.timestamp()), dt.timestamp_subsec_nanos())
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use hex_literal::hex;

    #[test]
    fn negative_is_none() {
        let value = Timestamp::from_secs_since_ntp_epoch(-1.0);
        assert_eq!(value, None);
    }

    #[test]
    fn above_max_errors() {
        let time_stamp = DateTime::<Utc>::from_timestamp(u32::MAX as i64 + 1, 0).unwrap();
        let value = Timestamp::from_utc_system(time_stamp);
        assert_eq!(value, None);
    }

    #[test]
    fn seconds() {
        let secs = 15;
        let value = Timestamp::from_secs_since_ntp_epoch(secs as f64).unwrap();

        assert_eq!(value.inner, secs << 32);
    }

    #[rstest::rstest]
    #[case::minimal(1.0 / (1u64 << 33) as f64, 1)]
    #[case::maximal(1.0 - 1.0 / ( 1u64 << 33) as f64, 1 << 32)]
    fn fraction(#[case] secs: f64, #[case] expected_inner: u64) {
        let value = Timestamp::from_secs_since_ntp_epoch(secs).unwrap();
        assert_eq!(value.inner, expected_inner);
    }

    #[test]
    fn to_secs() {
        let ntp_value = Timestamp { inner: 3 << 32 };
        assert_eq!(ntp_value.to_time_delta_since_ntp_epoch().num_seconds(), 3);
    }

    // Jan 29, 2025 22:28:14.000004228 UTC
    // seconds since epoch:  1738189694
    // nanoseconds: 4228
    const TIMESTAMP: &[u8] = &hex!("eb4529fe000046f3");
    const SECONDS: u64 = 1738189694;
    const NANOSECONDS: i32 = 4228;

    #[test]
    fn parse_from_packet() {
        let (leftover, parsed) = Timestamp::parse(TIMESTAMP).unwrap();
        assert!(leftover.is_empty());
        let secs = parsed.to_time_delta_since_unix_epoch();
        assert_eq!(secs.num_seconds() as u64, SECONDS);
        let error = secs.subsec_nanos() - NANOSECONDS;
        assert!(error.abs() < 1, "{error} {secs}")
    }

    #[test]
    fn from_clock_bound_instant() {
        let (_, timestamp) = Timestamp::parse(TIMESTAMP).unwrap();
        let datetime = timestamp.to_utc_datetime();
        let instant = ClockBoundInstant::from(timestamp);

        assert_eq!(instant.as_seconds() as i64, datetime.timestamp());
    }
}
