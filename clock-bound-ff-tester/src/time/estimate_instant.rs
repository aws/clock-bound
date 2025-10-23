//! The corresponding local view of time based off of a hardware timer
//!
//! All time readings/measurements have an error bound. This is a type to designate a time which has
//! come from an external reading and may have known inaccuracies.
//!
//! A linux "system time" is an "estimate time" in this definition. However, a linux system time is a linux specific clock,
//! and this crate uses a different name to prevent confusion from that term. For example, a TSC reading,
//! multiplied by its current frequency and start time creates another estimate time. But that time is not the same as the
//! linux system time.

use super::inner::{Diff, Time};
use clock_bound::daemon::time as cb_time;

/// Marker type to create a local timestamp with [`crate::time::Time`]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Estimate;

impl super::inner::Type for Estimate {}
impl super::inner::FemtoType for Estimate {}

/// Representation of an absolute time timestamp
///
/// This value represents the number of nanoseconds since epoch, without leap seconds
///
/// This type's epoch is January 1, 1970 0:00:00 UTC (aka "UNIX timestamp")
///
/// This type's inner value is an i128 number of nanoseconds from epoch.
pub type EstimateInstant = Time<Estimate>;

/// The corresponding duration type for [`EstimateInstant`]
pub type EstimateDuration = Diff<Estimate>;

pub trait CbBridge {
    type Estimate;
    fn into_estimate(self) -> Self::Estimate;
    fn from_estimate(estimate: Self::Estimate) -> Self;
}

impl CbBridge for cb_time::Instant {
    type Estimate = EstimateInstant;

    fn into_estimate(self) -> Self::Estimate {
        EstimateInstant::new(self.get())
    }

    fn from_estimate(estimate: Self::Estimate) -> Self {
        cb_time::Instant::new(estimate.get())
    }
}

impl CbBridge for cb_time::Duration {
    type Estimate = EstimateDuration;

    fn into_estimate(self) -> Self::Estimate {
        EstimateDuration::new(self.get())
    }

    fn from_estimate(estimate: Self::Estimate) -> Self {
        cb_time::Duration::new(estimate.get())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cb_bridge_instant() {
        let cb_instant = cb_time::Instant::new(123456789);
        let estimate_instant: EstimateInstant = cb_instant.into_estimate();
        assert_eq!(cb_instant.get(), estimate_instant.get());
        let cb_instant2 = cb_time::Instant::from_estimate(estimate_instant);
        assert_eq!(cb_instant, cb_instant2);
    }

    #[test]
    fn cb_bridge_duration() {
        let cb_duration = cb_time::Duration::new(123456789);
        let estimate_duration: EstimateDuration = cb_duration.into_estimate();
        assert_eq!(cb_duration.get(), estimate_duration.get());
        let cb_duration2 = cb_time::Duration::from_estimate(estimate_duration);
        assert_eq!(cb_duration, cb_duration2);
    }
}
