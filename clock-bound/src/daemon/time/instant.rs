//! A simplified time type for `ClockBound`

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
