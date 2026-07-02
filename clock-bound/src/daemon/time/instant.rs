//! A simplified time type for `ClockBound`
use nix::sys::time::TimeSpec;
use serde::{Deserialize, Serialize};

use super::inner::{Diff, Time};

/// Marker type to signify a time as a timestamp
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
pub struct Utc;

impl super::inner::Type for Utc {}
impl super::inner::NanoType for Utc {
    const INSTANT_PREFIX: &'static str = "Instant";
    const DURATION_PREFIX: &'static str = "Duration";
}

/// Representation of an absolute time timestamp
///
/// This value represents the number of **nano**seconds since epoch, without leap seconds.
/// A nanosecond is 1/1,000,000,000th of a second, or 1e-9
///
/// This type's epoch is January 1, 1970 0:00:00 UTC (aka "UNIX timestamp")
///
/// This type's inner value is an i64 number of nanoseconds from epoch.
pub type Instant = Time<Utc>;

/// The corresponding duration type for [`Instant`]
pub type Duration = Diff<Utc>;

impl From<Instant> for TimeSpec {
    fn from(value: Instant) -> Self {
        let seconds = value.as_seconds_trunc();
        let nanoseconds = value.as_nanos_trunc() % 1_000_000_000;
        TimeSpec::new(seconds, nanoseconds)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Instant::from_nanos(0), TimeSpec::new(0, 0))]
    #[case(Instant::from_nanos(1), TimeSpec::new(0, 1))]
    #[case(Instant::from_nanos(1_000), TimeSpec::new(0, 1_000))]
    #[case(Instant::from_nanos(1_000_000_000), TimeSpec::new(1, 0))]
    fn timespec_from_instant(#[case] instant: Instant, #[case] expected: TimeSpec) {
        assert_eq!(TimeSpec::from(instant), expected);
    }

    // Test to confirm that extreme values are representable as TimeSpec without panicking
    #[rstest]
    #[case::zero(Instant::new(i64::MIN))]
    #[case::max(Instant::new(i64::MAX))]
    fn timespec_from_instant_should_not_panic(#[case] instant: Instant) {
        let _ = TimeSpec::from(instant);
    }
}
