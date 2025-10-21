//! VMClock support

use crate::time::{EstimateDuration, EstimateInstant, Period, TscCount};
use serde::{Deserialize, Serialize};

/// A VMClock with time support
///
/// Represents the time and clock frequency values passed from the linux hypervisor.
///
/// For more info see:
/// <https://github.com/torvalds/linux/blob/master/include/uapi/linux/vmclock-abi.h>
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct VMClock {
    /// The TSC counter value at [`vmclock_time`](Self::vmclock_time)
    pub tsc_timestamp: TscCount,
    /// The clock period estimate at the time of [`tsc_timestamp`](Self::tsc_timestamp) in seconds
    pub clock_period: Period,
    /// The maximum error value of the [`clock_period`](Self::clock_period) value in seconds
    ///
    /// Can be None if VMClock does not support this value
    pub clock_period_max_error: Option<Period>,
    /// The timestamp (nanoseconds since epoch)
    pub vmclock_time: EstimateInstant,
    /// The maximum error of the [`vmclock_time`](Self::vmclock_time) value
    ///
    /// This value is expected to be None while `ClockBound 3.0` is in alpha
    pub vmclock_time_max_error: Option<EstimateDuration>,
    /// A unique identifier
    pub source_id: String,
}
