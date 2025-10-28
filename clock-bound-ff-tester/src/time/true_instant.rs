//! Everything is time. True time is the largely fictional/aspirational concept of what if we knew exactly what time it was, without error

use super::inner::{FemtoType, Type};

use crate::time::inner::{Diff, Time};

use super::{EstimateDuration, EstimateInstant};

/// Marker type to signify a time as True Time
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct True;

impl Type for True {}
impl FemtoType for True {
    const INSTANT_PREFIX: &'static str = "TrueInstant";
    const DURATION_PREFIX: &'static str = "TrueDuration";
}

/// Representation of true time timestamps
///
/// True time is, effectively, what all NTP clients are trying to determine. However getting an exact value
/// is impossible, as all things in life have an error (a Clock Error Bound in our terminology).
///
/// When `ff-tester` is creating simulations, `ff-tester` is able to know what "true time" is for a scenario because
/// it is not constrained by the rules of the real world.
///
/// This type's epoch is UTC. January 1, 1970 0:00:00 UTC (aka “UNIX timestamp”). Counted **without** leap seconds
///
/// This type's inner value is an i128 number of **femto**seconds from epoch.
pub type TrueInstant = Time<True>;

/// The corresponding duration type for [`TrueInstant`]
pub type TrueDuration = Diff<True>;

pub trait AssumeTrue {
    type TrueType;
    fn assume_true(self) -> Self::TrueType;
}

pub trait DemoteToEstimate {
    type EstimateType;
    fn demote_to_estimate(self) -> Self::EstimateType;
}

impl AssumeTrue for EstimateInstant {
    type TrueType = TrueInstant;

    fn assume_true(self) -> Self::TrueType {
        TrueInstant::new(self.get())
    }
}

impl AssumeTrue for EstimateDuration {
    type TrueType = TrueDuration;

    fn assume_true(self) -> Self::TrueType {
        TrueDuration::new(self.get())
    }
}

impl DemoteToEstimate for TrueInstant {
    type EstimateType = EstimateInstant;

    fn demote_to_estimate(self) -> Self::EstimateType {
        EstimateInstant::new(self.get())
    }
}

impl DemoteToEstimate for TrueDuration {
    type EstimateType = EstimateDuration;

    fn demote_to_estimate(self) -> Self::EstimateType {
        EstimateDuration::new(self.get())
    }
}
