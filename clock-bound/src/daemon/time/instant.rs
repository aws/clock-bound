//! A simplified time type for `ClockBound`
use nix::sys::time::TimeSpec;

use super::inner::{Diff, Time};

/// Marker type to signify a time as a timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Utc;

impl super::inner::Type for Utc {}
impl super::inner::FemtoType for Utc {
    const INSTANT_PREFIX: &'static str = "Instant";
    const DURATION_PREFIX: &'static str = "Duration";
}

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

impl TryFrom<Instant> for TimeSpec {
    // Reuse inner error from failure to convert i128 to i64
    type Error = <i64 as TryFrom<i128>>::Error;

    fn try_from(value: Instant) -> Result<Self, Self::Error> {
        let seconds = value.as_seconds_trunc().try_into()?;
        // Unwrap safety: 1e9 fits into i64
        let nanoseconds = (value.as_nanos_trunc() % 1_000_000_000).try_into().unwrap();
        Ok(TimeSpec::new(seconds, nanoseconds))
    }
}

#[cfg(test)]
mod test {
    use crate::daemon::time::{Clock, clocks::MonotonicCoarse};

    use super::*;
    use rstest::rstest;

    #[rstest]
    #[case(Instant::from_nanos(0), TimeSpec::new(0, 0))]
    #[case(Instant::from_nanos(1), TimeSpec::new(0, 1))]
    #[case(Instant::from_nanos(1_000), TimeSpec::new(0, 1_000))]
    #[case(Instant::from_nanos(1_000_000_000), TimeSpec::new(1, 0))]
    #[case::min_val(Instant::from_secs(i64::MIN as i128), TimeSpec::new(i64::MIN, 0))]
    #[case::max_val(Instant::from_secs(i64::MAX as i128), TimeSpec::new(i64::MAX, 0))]
    fn timespec_try_from_instant(#[case] instant: Instant, #[case] expected: TimeSpec) {
        assert_eq!(TimeSpec::try_from(instant).unwrap(), expected);
    }

    // Cheeky test to tell us that +/- 1 million years we are safe
    #[rstest]
    #[case::past(MonotonicCoarse.get_time() - Duration::from_days(365_000_000))]
    #[case::future(MonotonicCoarse.get_time() + Duration::from_days(365_000_000))]
    fn timespec_try_from_instant_should_not_panic(#[case] instant: Instant) {
        TimeSpec::try_from(instant).unwrap();
    }

    #[rstest]
    #[case::underflow(Instant::from_secs(i64::MIN as i128 - 1))]
    #[case::overflow(Instant::from_secs(i64::MAX as i128 + 1))]
    fn timespec_try_from_instant_failure(#[case] instant: Instant) {
        TimeSpec::try_from(instant).unwrap_err();
    }
}
